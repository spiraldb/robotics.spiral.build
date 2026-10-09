//! The robot connector: an HTTP endpoint that runs next to the arms it drives.
//!
//! It owns everything physical — serial buses, cameras, calibration, the
//! fixed-rate control loop — along with the takes recorded from it and the
//! policies rolled out on it. Studio is its browser frontend: the browser talks
//! to this server directly, carries finished recordings to Studio, and writes
//! the policies Studio trained here. The connector never reaches Spiral.
//!
//! This library is the connector itself; the program around it supplies the
//! machine's device drivers ([`DeviceDrivers`]) — a computer's USB serial ports,
//! webcams, and FFmpeg, or a microcontroller's UARTs and hardware encoder.

#![allow(async_fn_in_trait)]

mod clock;
mod context;
mod error;
mod http;
mod policies;
mod recording;
mod robotics;
mod robots;
mod rollout;
#[cfg(feature = "simulation")]
mod simulation;
mod sync;

pub(crate) use error::OperationError;
pub use http::server::{ConnectorSettings, DeviceDrivers, RobotConnector};
pub use robotics::cameras::{
    CameraDevice, CameraDriver, DEFAULT_JPEG_QUALITY, DiscoveredCameras, NoCameraDriver,
};
pub use robotics::video::EncoderDriver;
