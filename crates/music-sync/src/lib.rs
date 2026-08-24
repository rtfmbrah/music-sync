//! Core library for music-sync.
//!
//! The library owns domain behavior and external boundaries. Terminal rendering
//! belongs to the separate CLI crate.

#![forbid(unsafe_code)]

pub mod acquisition;
pub mod adoption;
pub mod config;
pub mod content_hash;
pub mod diagnostics;
pub mod identity;
pub mod media_probe;
pub mod persistence;
pub mod playlist;
pub mod preservation;
pub mod provider;
pub mod yt_dlp;
