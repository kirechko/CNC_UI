//! Statement execution and run lifecycle for the interpreter.

use std::collections::HashMap;

use crate::error::{RosettaError, RosettaResult};
use crate::ir::macro_ir::{Expr, MacroProgram, ParamDecl, Stmt, Value};
use crate::macro_engine::runtime::{
    CancellationToken, HostRuntime, MachineState, MacroLogLevel, MacroLogLine, RunOutcome,
};

use super::evaluation::default_for;
use super::ToDisplayString;
use super::{Engine, Exec, Signal, TransformHook, MAX_DEPTH, MAX_STEPS, MAX_WAIT_POLLS};

impl<'a, 'h> Engine<'a, 'h> {
    /// Create an engine; does not execute anything yet.
    pub(super) fn new(
        program: &'a MacroProgram,
        rt: &'a mut dyn HostRuntime,
        cancel: &'a CancellationToken,
        log: &'a mut dyn FnMut(MacroLogLine),
        transform: &'a mut Option<&'h mut TransformHook<'h>>,
    ) -> Self {
        Engine {
            program,
            rt,
            cancel,
            log,
            transform,
            scopes: vec![HashMap::new()],
            depth: 0,
            steps: 0,
            pending_return: None,
        }
    }

    /// Entry point: bind parameters, seed locals, run the body (§6.3:
    /// `Idle → Running → Done/Aborted`).
    pub(super) fn run(
        program: &'a MacroProgram,
        args: Vec<Value>,
        rt: &'a mut dyn HostRuntime,
        cancel: &'a CancellationToken,
        log: &'a mut dyn FnMut(MacroLogLine),
        transform: &'a mut Option<&'h mut TransformHook<'h>>,
    ) -> RosettaResult<RunOutcome> {
        let mut engine = Engine::new(program, rt, cancel, log, transform);
        // Bind positional parameters to declared names; missing arguments
        // take the declared default or the typed zero value.
        for (index, param) in program.params.iter().enumerate() {
            let value = engine.bind_argument(args.get(index), param)?;
            engine.declare(&param.name, value);
        }
        for local in &program.locals {
            let value = match &local.initial {
                Some(expr) => engine.eval(expr)?,
                None => default_for(local.kind),
            };
            engine.declare(&local.name, value);
        }
        engine.log_line("start", MacroLogLevel::Info);
        let block = program.body.clone();
        match engine.exec_block(&block) {
            Ok(()) => Ok(RunOutcome::Completed {
                value: engine.pending_return,
            }),
            Err(Signal::Return(value)) => Ok(RunOutcome::Completed { value: Some(value) }),
            Err(Signal::Abort(reason)) => {
                engine.log_line(&format!("abort: {reason}"), MacroLogLevel::Error);
                Ok(RunOutcome::Aborted { reason })
            }
            Err(Signal::Cancelled) => {
                engine.log_line("cancelled", MacroLogLevel::Warn);
                Ok(RunOutcome::Cancelled)
            }
            Err(Signal::Error(error)) => Err(error),
        }
    }

    /// Resolve one parameter binding: explicit arg → default expr → zero.
    fn bind_argument(
        &mut self,
        provided: Option<&Value>,
        param: &ParamDecl,
    ) -> RosettaResult<Value> {
        match provided {
            Some(value) => Ok(value.clone()),
            None => match &param.default {
                Some(default) => self.eval(default),
                None => Ok(default_for(param.kind)),
            },
        }
    }

    // ----- variable scope helpers -----------------------------------------

    /// Write a variable into the innermost scope declaring it, else the
    /// current scope — dynamic-scope semantics analogous to `#<name>`.
    pub(super) fn set_var(&mut self, name: &str, value: Value) {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_owned(), value);
                return;
            }
        }
        self.scopes
            .last_mut()
            .expect("engine always keeps the global scope")
            .insert(name.to_owned(), value);
    }

    /// Declare a fresh variable in the current (top) scope.
    pub(super) fn declare(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("engine always keeps the global scope")
            .insert(name.to_owned(), value);
    }

    /// Read a variable, walking scopes innermost-out (`Expr::Var`).
    /// Unknown names are hard errors — silent zeros would distort motion.
    pub(super) fn get_var(&self, name: &str) -> RosettaResult<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Ok(value.clone());
            }
        }
        Err(RosettaError::Validation {
            reason: format!("undefined variable \"{name}\""),
        })
    }

    // ----- logging ---------------------------------------------------------

    /// Emit one `[MACRO]` log line (ТЗ №2 §10).
    pub(super) fn log_line(&mut self, message: &str, level: MacroLogLevel) {
        let line = MacroLogLine {
            macro_id: self.program.meta.id.clone(),
            step: Some((self.steps, None)),
            message: message.to_owned(),
            level,
        };
        (self.log)(line);
    }

    // ----- statement execution --------------------------------------------

    /// Execute a block sequentially; stops on the first error or signal.
    pub(super) fn exec_block(&mut self, block: &[Stmt]) -> Exec<()> {
        for stmt in block {
            self.exec_stmt(stmt)?;
        }
        Ok(())
    }

    /// Execute one statement, honoring cancellation, step budget, and the
    /// alarm/E-Stop abort rules (§6.2, §6.4, §9.1).
    pub(super) fn exec_stmt(&mut self, stmt: &Stmt) -> Exec<()> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(Signal::Error(RosettaError::Validation {
                reason: format!("macro exceeded the {MAX_STEPS}-step budget"),
            }));
        }
        if self.cancel.is_cancelled() {
            return Err(Signal::Cancelled);
        }
        match stmt {
            Stmt::Comment { .. } => Ok(()),
            Stmt::GCode { template } => {
                let command = self.interpolate(template).map_err(Signal::Error)?;
                self.log_line(&format!("send {command}"), MacroLogLevel::Info);
                let response = self.rt.send_command(&command).map_err(Signal::Error)?;
                // A command that drove the machine into alarm/E-Stop aborts
                // the run immediately (§6.2 "прерывается немедленно").
                let state = self.rt.machine_state();
                if state.must_abort() {
                    self.log_line(
                        &format!("machine entered {state:?} during {command}"),
                        MacroLogLevel::Error,
                    );
                    return Err(if state == MachineState::EStop {
                        Signal::Cancelled
                    } else {
                        Signal::Error(RosettaError::Runtime {
                            reason: format!("controller alarm after {command}: {response}"),
                        })
                    });
                }
                Ok(())
            }
            Stmt::Delay { ms } => {
                let ms = self.eval(ms).map_err(Signal::Error)?.as_num().max(0.0) as u64;
                self.log_line(&format!("delay {ms}ms"), MacroLogLevel::Info);
                self.rt.sleep_ms(ms);
                Ok(())
            }
            Stmt::Wait { cond } => self.exec_wait(cond),
            Stmt::SetVar { name, value } => {
                let value = self.eval(value).map_err(Signal::Error)?;
                self.set_var(name, value);
                Ok(())
            }
            Stmt::If {
                cond,
                then,
                otherwise,
            } => {
                let taken = self.eval(cond).map_err(Signal::Error)?.is_truthy();
                self.log_line(&format!("condition -> {taken}"), MacroLogLevel::Info);
                self.exec_block(if taken { then } else { otherwise })
            }
            Stmt::While { cond, body } => loop {
                if self.cancel.is_cancelled() {
                    return Err(Signal::Cancelled);
                }
                if !self.eval(cond).map_err(Signal::Error)?.is_truthy() {
                    return Ok(());
                }
                self.exec_block(body)?;
            },
            Stmt::Repeat { count, body } => {
                let count = self.eval(count).map_err(Signal::Error)?.as_num();
                if !count.is_finite() || count < 0.0 || count.fract() != 0.0 {
                    return Err(Signal::Error(RosettaError::Validation {
                        reason: format!("repeat count must be a non-negative integer, got {count}"),
                    }));
                }
                for _ in 0..count as u64 {
                    if self.cancel.is_cancelled() {
                        return Err(Signal::Cancelled);
                    }
                    self.exec_block(body)?;
                }
                Ok(())
            }
            Stmt::ForEach { var, iter, body } => {
                let items = self.expand_iter(iter).map_err(Signal::Error)?;
                for item in items {
                    if self.cancel.is_cancelled() {
                        return Err(Signal::Cancelled);
                    }
                    self.set_var(var, item);
                    self.exec_block(body)?;
                }
                Ok(())
            }
            Stmt::Call { name, args } => self.exec_call(name, args),
            Stmt::InvokeTransform { id, args } => {
                let values = self.eval_args(args).map_err(Signal::Error)?;
                self.log_line(&format!("transform {id}"), MacroLogLevel::Info);
                // MAC-T-03/05: synchronous call into the transform engine.
                let result = match &mut *self.transform {
                    Some(hook) => hook(id.as_str(), values).map_err(Signal::Error)?,
                    None => {
                        return Err(Signal::Error(RosettaError::NotFound {
                            kind: "transform bridge".into(),
                            id: id.clone(),
                        }))
                    }
                };
                // MAC-T-06: cancellation raised during the transform stops
                // the macro before any further motion.
                if self.cancel.is_cancelled() {
                    return Err(Signal::Cancelled);
                }
                if let Some(value) = result {
                    self.pending_return = Some(value.clone());
                    self.set_var("return", value);
                }
                Ok(())
            }
            Stmt::Return { value } => {
                let value = match value {
                    Some(expr) => self.eval(expr).map_err(Signal::Error)?,
                    None => Value::Null,
                };
                Err(Signal::Return(value))
            }
            Stmt::Abort { reason } => {
                let reason = self
                    .eval(reason)
                    .map_err(Signal::Error)?
                    .to_display_string();
                Err(Signal::Abort(reason))
            }
        }
    }

    /// `Stmt::Wait`: poll the condition until truthy, bounded, with the same
    /// cancel/alarm rules as any loop (ТЗ №2 §6.5 reactive path).
    pub(super) fn exec_wait(&mut self, cond: &Expr) -> Exec<()> {
        for poll in 0..MAX_WAIT_POLLS {
            if self.cancel.is_cancelled() {
                return Err(Signal::Cancelled);
            }
            if self.eval(cond).map_err(Signal::Error)?.is_truthy() {
                return Ok(());
            }
            let state = self.rt.machine_state();
            if state.must_abort() {
                return Err(Signal::Error(RosettaError::Runtime {
                    reason: format!("machine in {state:?} while waiting"),
                }));
            }
            if poll == 0 {
                self.log_line("wait: polling status…", MacroLogLevel::Info);
            }
            self.rt.sleep_ms(50);
        }
        Err(Signal::Error(RosettaError::Runtime {
            reason: format!("wait condition did not become true within {MAX_WAIT_POLLS} polls"),
        }))
    }

    /// `Stmt::Call`: push a scope, bind parameters, execute the sub body,
    /// pop. A `Return` from the sub is consumed here (captured as `"return"`)
    /// so only a *top-level* Return can end the whole macro.
    pub(super) fn exec_call(&mut self, name: &str, args: &[Expr]) -> Exec<()> {
        // Copy the sub body out of `self.program` so execution can mutate
        // scopes while holding no borrow of the program itself.
        let body = self
            .program
            .find_sub(name)
            .ok_or_else(|| {
                Signal::Error(RosettaError::NotFound {
                    kind: "subroutine".into(),
                    id: name.to_owned(),
                })
            })?
            .clone();
        if self.depth + 1 > MAX_DEPTH {
            return Err(Signal::Error(RosettaError::Validation {
                reason: format!("subroutine nesting exceeds MAX_DEPTH ({MAX_DEPTH})"),
            }));
        }
        let values = self.eval_args(args).map_err(Signal::Error)?;
        self.scopes.push(HashMap::new());
        self.depth += 1;
        for (index, param) in body.params.iter().enumerate() {
            let value = match values.get(index) {
                Some(value) => value.clone(),
                None => match &param.default {
                    Some(default) => self.eval(default).map_err(Signal::Error)?,
                    None => default_for(param.kind),
                },
            };
            self.declare(&param.name, value);
        }
        let stmts = body.body.clone();
        let result = self.exec_block(&stmts);
        self.depth -= 1;
        self.scopes.pop();
        match result {
            Ok(()) => Ok(()),
            Err(Signal::Return(value)) => {
                self.pending_return = Some(value.clone());
                self.set_var("return", value);
                Ok(())
            }
            Err(signal) => Err(signal),
        }
    }
}
