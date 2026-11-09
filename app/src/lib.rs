//! Whodunit's app logic: the pi RPC client, the transcript model, the scoreboard and the views.

pub mod config;
pub mod core;
pub mod fixtures;
pub mod folders;
pub mod menu_bar;
pub mod pi;
pub mod scoreboard;
pub mod sessions;
pub mod sprites;
pub mod transcript;
pub mod ui {
    pub mod dither;
    pub mod folder_picker;
    pub mod full;
    pub mod parts;
}
