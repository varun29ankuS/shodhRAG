//! Where and how fast the models run: the threads a CPU session uses
//! ([`cores`]), the graphics card or CPU that runs the search model
//! ([`provider`], [`ep`]), its measured speed ([`speed`], [`calibration`]) and what
//! the app shows about it ([`status`]).

pub mod calibration;
pub mod cores;
pub mod ep;
pub mod provider;
pub mod speed;
pub mod status;
