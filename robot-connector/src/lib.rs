//! The robot connector: the program that runs next to the arms it drives.
//!
//! It owns everything physical — serial buses, cameras, calibration, the
//! fixed-rate control loop — along with the takes recorded from it. It links
//! itself to Studio over plain HTTP
//! ([`StudioLink`]): it polls Studio for the browser requests to answer and
//! posts live data while someone watches, so it needs no public address. The
//! browser reaches it only through Studio. The connector never reaches Spiral.
//!
//! This library is the connector itself; the program around it supplies the
//! machine's device drivers ([`DeviceDrivers`]) — a computer's USB serial
//! ports, webcams, FFmpeg, and network, or a microcontroller's UARTs, hardware
//! encoder, and Wi-Fi.

#![allow(async_fn_in_trait)]

mod context;
mod error;
mod http;
mod link;
mod recording;
mod robotics;
mod robots;
#[cfg(feature = "simulation")]
mod simulation;
mod sync;

pub(crate) use error::OperationError;
pub use http::server::{DeviceDrivers, RobotConnector};
pub use link::{LinkDriver, StudioLink};
pub use robotics::cameras::{
    CameraDevice, CameraDriver, DEFAULT_JPEG_QUALITY, DiscoveredCameras, NoCameraDriver,
};
pub use robotics::video::EncoderDriver;
