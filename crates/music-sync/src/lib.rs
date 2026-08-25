//! Core library for music-sync.
//!
//! The library owns domain behavior and external boundaries. Terminal rendering
//! belongs to the separate CLI crate.

#![forbid(unsafe_code)]

pub mod acquisition;
pub mod adoption;
pub mod adoption_link;
pub mod artwork;
pub mod config;
pub mod content_hash;
pub mod diagnostics;
pub mod discovery;
pub mod discovery_routing;
pub mod fingerprint;
pub mod health;
pub mod identity;
pub mod listenbrainz;
pub mod lyrics;
pub mod media_probe;
pub mod metadata;
pub mod musicbrainz;
pub mod navidrome;
pub mod persistence;
pub mod playlist;
pub mod preservation;
pub mod provider;
pub mod repair;
pub mod service;
pub mod sync;
pub mod tag_materialization;
pub mod yt_dlp;
