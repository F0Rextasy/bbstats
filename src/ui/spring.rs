//! Damped spring integrator for 60/120fps morph transitions with automatic idle settle.

use crate::render::theme::SpringSpec;

#[derive(Debug, Clone, Copy)]
pub struct SpringVal {
    pub value: f32,
    pub target: f32,
    pub velocity: f32,
}

impl SpringVal {
    pub const fn new(initial: f32) -> Self {
        Self {
            value: initial,
            target: initial,
            velocity: 0.0,
        }
    }

    pub fn snap(&mut self, val: f32) {
        self.value = val;
        self.target = val;
        self.velocity = 0.0;
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    fn advance_substeps(&mut self, dt: f32, spec: SpringSpec) {
        let clamped_dt = dt.clamp(0.001, 0.033);
        let sub_dt = clamped_dt * 0.25;
        for _ in 0..4 {
            let disp = self.value - self.target;
            let accel = (-spec.stiffness * disp - spec.damping * self.velocity) / spec.mass;
            self.velocity += accel * sub_dt;
            self.value += self.velocity * sub_dt;
        }
    }

    fn settle_check(&mut self, pos_eps: f32, vel_eps: f32) -> bool {
        if (self.value - self.target).abs() < pos_eps && self.velocity.abs() < vel_eps {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }

    /// Advances a pixel/DIP spring by `dt` seconds using 4 sub-steps for phase stability.
    pub fn step(&mut self, dt: f32, spec: SpringSpec) -> bool {
        self.advance_substeps(dt, spec);
        self.settle_check(0.30, 1.2)
    }

    /// Advances a normalized `[0..1]` scalar spring (`morph_t`, `bg_alpha`) with tight settle tolerance.
    pub fn step_unit(&mut self, dt: f32, spec: SpringSpec) -> bool {
        self.advance_substeps(dt, spec);
        self.settle_check(0.004, 0.02)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::theme::SPRING_MORPH;

    #[test]
    fn critically_damped_spring_settles_monotonically_without_overshoot() {
        for dt in [0.016f32, 0.031f32] {
            let mut open = SpringVal::new(30.0);
            open.set_target(720.0);
            let mut steps = 0;
            while open.step(dt, SPRING_MORPH) {
                assert!(open.value <= 720.001, "expand overshoot at dt={dt}: {}", open.value);
                steps += 1;
                assert!(steps <= 50, "expand took too long at dt={dt}");
            }
            assert_eq!(open.value, 720.0);
            assert!(steps <= 45, "expand steps={steps} at dt={dt}");

            let mut close = SpringVal::new(720.0);
            close.set_target(30.0);
            let mut csteps = 0;
            while close.step(dt, SPRING_MORPH) {
                assert!(close.value >= 29.999, "collapse overshoot at dt={dt}: {}", close.value);
                csteps += 1;
                assert!(csteps <= 50, "collapse took too long at dt={dt}");
            }
            assert_eq!(close.value, 30.0);
            assert!(csteps <= 45, "collapse steps={csteps} at dt={dt}");
        }
    }
}
