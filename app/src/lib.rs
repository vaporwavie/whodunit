//! Whodunit's app logic: the pi RPC client, the transcript model, the scoreboard and the views.

pub mod config;
pub mod core;
pub mod fixtures;
pub mod folders;
pub mod menu_bar;
pub mod pi;
pub mod scoreboard;
pub mod service;
pub mod sessions;
pub mod sprites;
pub mod transcript;
pub mod ui {
    pub mod design_system;
    pub mod develop;
    pub mod dither;
    pub mod parts;
    pub mod seal;
    pub mod stage;
    pub mod window;
}
