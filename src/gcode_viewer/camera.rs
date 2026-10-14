 //! Camera orientation math and projection helpers for the G-code viewer.
use super::types::CameraOrientation;

impl CameraOrientation {
    pub(super) fn new() -> Self {
        // Изометрический вид: камера смотрит на сцену с направления (1, -1, 1).
        let mut orientation = Self {
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fwd: [0.0, 0.0, 1.0],
        };
        orientation.rotate([1.0, 0.0, 0.0], 35.264_f64.to_radians());
        orientation.rotate([0.0, 0.0, 1.0], 45.0_f64.to_radians());
        orientation
    }

    pub(super) fn rotate(&mut self, axis: [f64; 3], theta: f64) {
        let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
        if length < 1e-12 {
            return;
        }

        let x = axis[0] / length;
        let y = axis[1] / length;
        let z = axis[2] / length;
        let c = theta.cos();
        let s = theta.sin();
        let t = 1.0 - c;

        let m = [
            [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
            [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
            [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
        ];

        for v in [&mut self.right, &mut self.up, &mut self.fwd] {
            let old = *v;
            *v = [
                m[0][0] * old[0] + m[0][1] * old[1] + m[0][2] * old[2],
                m[1][0] * old[0] + m[1][1] * old[1] + m[1][2] * old[2],
                m[2][0] * old[0] + m[2][1] * old[1] + m[2][2] * old[2],
            ];
        }
    }

    pub(super) fn project(
        &self,
        x: f64,
        y: f64,
        z: f64,
        pan_x: f64,
        pan_y: f64,
        scale: f64,
        width: f64,
        height: f64,
        perspective: bool,
    ) -> (f64, f64) {
        let tx = self.right[0] * x + self.right[1] * y + self.right[2] * z;
        let ty = self.up[0] * x + self.up[1] * y + self.up[2] * z;
        let tz = self.fwd[0] * x + self.fwd[1] * y + self.fwd[2] * z;

        // Положительный tz направлен к камере. Ограничиваем знаменатель,
        // чтобы точки около плоскости камеры не давали бесконечную проекцию.
        let perspective_factor = if perspective {
            let camera_distance = 500.0;
            camera_distance / (camera_distance - tz).max(camera_distance * 0.1)
        } else {
            1.0
        };

        (
            tx * scale * perspective_factor + pan_x + width / 2.0,
            -ty * scale * perspective_factor + pan_y + height / 2.0,
        )
    }
}

pub(super) fn orientation_for_face(normal: [f64; 3]) -> CameraOrientation {
    let (right, up, fwd) = if normal[2] > 0.5 {
        // Front (+Z): Y направлена вправо, X — вниз на экране.
        ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
    } else if normal[2] < -0.5 {
        ([0.0, -1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, -1.0])
    } else if normal[0] > 0.5 {
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0])
    } else if normal[0] < -0.5 {
        ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0])
    } else if normal[1] > 0.5 {
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    } else {
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0])
    };

    CameraOrientation { right, up, fwd }
}
