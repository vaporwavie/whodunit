//! The sidebar: New task and Scoreboard, then every session grouped Working, Idle and Ended.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use rune::{El, Icon, ScrollSpec, col};
use rune_kit::shell::{RowStatus, Sidebar, SidebarHeader, SidebarRow, SidebarSection};
use rune_kit::{Kit, Tooltip};

use super::{Page, Window, rename, sprite};
use crate::config;
use crate::core::{Core, Leave, SessionState, cmd};
use crate::sessions::Summary;
use crate::sprites;

static PLUS: Icon = Icon {
    name: "aw-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
static CHART: Icon = Icon {
    name: "aw-chart",
    view: 16.0,
    body: r#"<path d="M3 13V8M8 13V3M13 13V6" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>"#,
};

/// One session in the sidebar; `names` are the agents' display names, read once per sidebar.
fn session_row(
    s: &Window,
    core: &Core,
    summary: &Summary,
    names: &BTreeMap<String, String>,
    k: &Kit,
    layer: &mut Vec<El>,
) -> El {
    let current = core.session_file.as_deref() == Some(summary.path.to_string_lossy().as_ref());
    let selected = current && s.page == Page::Chat;
    let path = summary.path.clone();
    let subtitle = summary.subtitle_at(SystemTime::now());
    let mut r = SidebarRow::new(summary.title.clone())
        .subtitle(subtitle.clone())
        .selected(selected)
        .on_select(move |s: &mut Window, tx| {
            if selected {
                rename::begin(s, rename::Place::Sidebar, tx);
            } else {
                s.leave(Leave::Open(path.clone()), tx);
            }
        });
    if selected && let Some(edit) = rename::at(s, rename::Place::Sidebar) {
        r = r.renaming(edit, rename::edit, rename::save, rename::cancel);
    }
    // The role's and the agent's sprites, only once the run has ended and they can show.
    // The role's sits in the title's line, so it drops to centre on both lines.
    let deal = summary.revealed();
    if let Some(deal) = deal {
        let file = summary.path.display();
        let role = sprite_tip(
            s,
            format!("role-{file}"),
            sprites::role_icon(&deal.role),
            deal.role.clone(),
            k,
            layer,
        );
        let agent = sprite_tip(
            s,
            format!("agent-{file}"),
            sprites::model_icon(&deal.agent),
            config::agent_name_in(names, &deal.agent),
            k,
            layer,
        );
        r = r.avatar(role.offset(0.0, ROLE_ICON_DROP)).trailing(agent);
    }
    if core.session_state(summary) == SessionState::Working {
        r = r.status(RowStatus::Busy);
    } else if core.tasks_waiting(summary) {
        r = r.status(RowStatus::Attention);
    }
    let mut r = r.build(k);
    if selected {
        r = r
            .hover(k.c.bg_accent.alpha(0.08))
            .cursor(rune::Cursor::Text);
    }
    if let Some(deal) = deal {
        r = r.label(format!(
            "{}, {}, {}, {}",
            summary.title,
            deal.role,
            config::agent_name_in(names, &deal.agent),
            subtitle
        ));
    }
    r.key(format!("session-{}", summary.path.display()))
}

/// Half the subtitle's line: the row puts its icon in the title's line, and the role icon
/// drops by this to centre on both lines.
const ROLE_ICON_DROP: f32 = 8.0;
pub(super) const SPRITE_TIP_DELAY: Duration = Duration::from_millis(500);

/// A sidebar sprite that names itself in a tooltip while the pointer rests on it. `key` names
/// the tooltip; the sprite itself is keyed `{key}-icon`.
fn sprite_tip(
    s: &Window,
    key: String,
    art: &sprites::Art,
    name: String,
    k: &Kit,
    layer: &mut Vec<El>,
) -> El {
    let id = key.clone();
    let icon = sprite(art)
        .key(format!("{key}-icon"))
        .hover_delays(SPRITE_TIP_DELAY, Duration::ZERO)
        .on_hover_change(move |s: &mut Window, inside, _| {
            if inside {
                s.sprite_tip = Some(id.clone());
            } else if s.sprite_tip.as_ref() == Some(&id) {
                s.sprite_tip = None;
            }
        });
    let open = s.sprite_tip.as_ref() == Some(&key);
    Tooltip::new(key)
        .trigger(icon)
        .text(k, name)
        .open(open)
        .build(k, layer)
}

pub(super) fn groups(
    core: &Core,
) -> impl Iterator<Item = (SessionState, &'static str, Vec<&Summary>)> {
    [
        (SessionState::Working, "Working"),
        (SessionState::Idle, "Idle"),
        (SessionState::Ended, "Ended"),
    ]
    .into_iter()
    .map(move |(state, label)| {
        let sessions = core
            .sessions
            .iter()
            .filter(|summary| core.session_state(summary) == state)
            .collect();
        (state, label, sessions)
    })
}

pub(super) fn build(s: &Window, core: &Core, k: &Kit, layer: &mut Vec<El>) -> El {
    // Only ended rows name their agent, so the file is read only while some show.
    let shown = |summary: &&Summary| {
        !s.ended_collapsed || core.session_state(summary) != SessionState::Ended
    };
    let names = if core
        .sessions
        .iter()
        .filter(shown)
        .any(|summary| summary.revealed().is_some())
    {
        config::agent_names()
    } else {
        BTreeMap::new()
    };
    let mut sb = Sidebar::new()
        .label("Sessions")
        .inset(0.0)
        .header_gap(2.0)
        .row_gap(12.0)
        .header(
            SidebarRow::new("New task")
                .icon(&PLUS)
                .detail(cmd("N"))
                .height(32.0)
                .on_select(|s: &mut Window, tx| {
                    s.leave(Leave::New, tx);
                    s.draft.composer.focused = true;
                })
                .build(k)
                .key("new"),
        )
        .header(
            SidebarRow::new("Scoreboard")
                .icon(&CHART)
                .detail(cmd("2"))
                .height(32.0)
                .selected(s.page == Page::Scoreboard)
                .on_select(|s: &mut Window, tx| s.open_scoreboard(tx))
                .build(k)
                .key("scoreboard"),
        );
    for (state, label, sessions) in groups(core) {
        if sessions.is_empty() {
            continue;
        }
        if state == SessionState::Ended {
            let mut ended = col()
                .key("Ended")
                .role("group")
                .label(label)
                .w_full()
                .gap(2.0)
                .child(
                    SidebarHeader::new(label)
                        .count(sessions.len())
                        .collapsed(s.ended_collapsed)
                        .on_toggle(|s: &mut Window, _| {
                            s.ended_collapsed = !s.ended_collapsed;
                            s.ended_cycle_expanded = false;
                        })
                        .build(k),
                );
            if !s.ended_collapsed {
                ended = ended.child(
                    col()
                        .key("ended-list")
                        .role("list")
                        .label("Ended sessions")
                        .focus_group()
                        .focus_follows()
                        .w_full()
                        .max_h(240.0)
                        .gap(2.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .scroll_edge_fade(16.0)
                        .children(
                            sessions
                                .iter()
                                .map(|summary| session_row(s, core, summary, &names, k, layer)),
                        ),
                );
            }
            sb = sb.footer(ended);
        } else {
            sb = sb.section(
                SidebarSection::new(label).count(sessions.len()).rows(
                    sessions
                        .iter()
                        .map(|summary| session_row(s, core, summary, &names, k, layer)),
                ),
            );
        }
    }
    sb.build(k)
}
