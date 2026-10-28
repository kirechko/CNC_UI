// Startup error handlers: shows UI initialization failures instead of leaving the "Connecting" state.
      // Show UI initialization errors instead of staying stuck on
      // the "Connecting…" state.
      window.addEventListener("error", (event) => {
        console.error("UI JavaScript error:", event.error || event.message);
        const connection = document.querySelector("#connection");
        if (connection) {
          const details = event.message || "JavaScript loading error";
          connection.textContent = `UI error: ${details}`;
          connection.title = details;
        }
      });

      window.addEventListener("unhandledrejection", (event) => {
        console.error("Unhandled UI error:", event.reason);
        const connection = document.querySelector("#connection");
        if (connection) {
          const details = String(event.reason?.message || event.reason);
          connection.textContent = `UI error: ${details}`;
          connection.title = details;
        }
      });
