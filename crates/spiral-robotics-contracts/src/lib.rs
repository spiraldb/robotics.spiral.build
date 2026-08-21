//! Delivery and persistence contracts shared by Spiral Robotics adapters.
//!
//! This crate depends inward on `spiral-robotics-core` for domain values. Core
//! never depends on these HTTP, browser-event, or SpiralDB representations.

pub mod api;
pub mod spiral;
pub mod workflow;
