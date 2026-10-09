//! Arms the connector simulates, and the cameras that film them.
//!
//! A simulated arm is an ordinary saved robot whose motors are simulated: the
//! control loop, calibration, and recording treat it exactly like a
//! real one. What a real bench gets from physical cameras, a simulated one gets
//! by drawing its arms, with the renderer the browser's 3D view uses, from
//! wherever each camera is mounted.

mod camera;
mod demo;
mod world;

pub(crate) use camera::SimulatedCamera;
pub(crate) use demo::DemoBench;
pub(crate) use world::SimulatedWorld;

#[cfg(test)]
mod tests;
