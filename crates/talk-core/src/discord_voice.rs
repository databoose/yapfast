//! Discord voice: a call through songbird with Talk's own audio, from asking
//! Discord to join until leaving.
use std::{path::Path, time::Duration};

use anyhow::{Result, bail, ensure};
use talk_discord::{
    service::{Discord, Status},
    voice::{Call as DiscordCall, live_microphone},
};
use tokio::sync::{mpsc, watch};

use crate::{
    discord_audio::{self, Capture, Hearing, Speakers},
    voice::{Chosen, Command, Participant, Phase, State, devices},
};

/// A Discord voice call, from asking Discord to join until leaving. Commands
/// stay responsive throughout, as in a Matrix call.
pub(crate) async fn call(
    discord: &Discord,
    guild: &str,
    channel: &str,
    state: &mut State,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    snapshots: &watch::Sender<State>,
    remembered: Option<&Path>,
) -> Result<()> {
    ensure!(
        discord_audio::AVAILABLE,
        "Discord voice works on Linux for now."
    );
    let mut discord_states = discord.snapshots();
    let me = discord_states.borrow().user_id.clone().unwrap_or_default();
    state.name = discord_states
        .borrow()
        .servers
        .iter()
        .flat_map(|server| &server.voice_channels)
        .find(|found| found.id == channel)
        .map_or_else(|| "Discord voice".into(), |found| found.name.clone());
    snapshots.send_replace(state.clone());

    let (ended, mut endings) = mpsc::unbounded_channel();
    let hearing = std::sync::Arc::new(Hearing::new(ended));
    let joining = async {
        let server = discord.join(guild, channel).await?;
        DiscordCall::connect(&server, guild, channel, hearing.clone()).await
    };
    tokio::pin!(joining);
    let deadline = tokio::time::sleep(Duration::from_secs(35));
    tokio::pin!(deadline);
    let joined = loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                None | Some(Command::Leave) => {
                    discord.leave(guild);
                    return Ok(());
                }
                // Used once the call connects.
                Some(Command::Microphone(id)) => state.microphone = Some(id),
                Some(Command::Speaker(id)) => state.speaker = Some(id),
                Some(Command::Devices) => { let _ = devices(state); }
                _ => {}
            },
            () = &mut deadline => {
                discord.leave(guild);
                bail!("Discord voice took too long to connect. Try again.");
            }
            result = &mut joining => break result,
        }
    };
    let mut call = match joined {
        Ok(call) => call,
        Err(error) => {
            discord.leave(guild);
            return Err(error);
        }
    };
    let (played, reverse) = std::sync::mpsc::sync_channel(8);
    let reverse = std::sync::Arc::new(std::sync::Mutex::new(reverse));
    let (microphone, input) = live_microphone();
    let track = call.play(input);
    let _ = track.pause();
    let mut capture: Option<Capture> = None;

    let result = async {
        let mut speakers = Some(open_discord_speakers(state, &played, &hearing)?);
        Chosen::write(remembered, state);
        state.phase = Phase::Connected;
        let mut ticks = tokio::time::interval(Duration::from_millis(150));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let talking = capture.as_ref().is_some_and(Capture::talking);
            let participants = discord_participants(
                &discord_states.borrow(),
                channel,
                &me,
                state,
                &hearing.speaking(),
                talking,
            );
            if participants != state.participants {
                state.participants = participants;
                snapshots.send_replace(state.clone());
            }
            tokio::select! {
                command = commands.recv() => {
                    match command {
                        None | Some(Command::Leave) => break,
                        Some(Command::Join(_)) => state.error = Some("Leave your current voice channel before joining another.".into()),
                        Some(Command::Mute(true)) => {
                            capture = None;
                            state.muted = true;
                        }
                        Some(Command::Mute(false)) => {
                            // Unmuting while deafened means wanting to talk, which needs hearing too.
                            if state.deafened {
                                state.deafened = false;
                                hearing.set_deafened(false);
                            }
                            match open_discord_microphone(state, &reverse, &microphone, &track) {
                                Ok(opened) => {
                                    capture = Some(opened);
                                    state.muted = false;
                                    state.error = None;
                                }
                                Err(error) => {
                                    state.muted = true;
                                    state.error = Some(error.to_string());
                                }
                            }
                        }
                        Some(Command::Deafen(deafened)) => {
                            if deafened {
                                capture = None;
                                state.muted = true;
                            }
                            state.deafened = deafened;
                            hearing.set_deafened(deafened);
                        }
                        Some(Command::Devices) => { let _ = devices(state); }
                        Some(Command::Microphone(id)) => {
                            state.microphone = Some(id);
                            Chosen::write(remembered, state);
                            if capture.take().is_some() {
                                match open_discord_microphone(state, &reverse, &microphone, &track) {
                                    Ok(opened) => capture = Some(opened),
                                    Err(_) => {
                                        state.muted = true;
                                        state.error = Some("Could not switch microphone. Refresh devices and choose another.".into());
                                    }
                                }
                            }
                        }
                        Some(Command::Speaker(id)) => {
                            state.speaker = Some(id);
                            Chosen::write(remembered, state);
                            speakers = None;
                            match open_discord_speakers(state, &played, &hearing) {
                                Ok(opened) => speakers = Some(opened),
                                Err(_) => state.error = Some("Could not switch speakers. Refresh devices and choose another.".into()),
                            }
                        }
                        Some(Command::LeaveStale) => {}
                    }
                    discord.set_self(guild, channel, state.muted, state.deafened);
                    snapshots.send_replace(state.clone());
                }
                Some(reason) = endings.recv() => {
                    tracing::warn!(%reason, "Discord voice disconnected");
                    bail!("Discord voice disconnected. Join again to reconnect.");
                }
                changed = discord_states.changed() => {
                    if changed.is_err() || discord_states.borrow().status != Status::Online {
                        bail!("Lost the connection to Discord. Join again when it's back.");
                    }
                }
                _ = ticks.tick() => {}
            }
        }
        drop(speakers);
        Ok(())
    }
    .await;
    state.phase = Phase::Leaving;
    snapshots.send_replace(state.clone());
    drop(capture);
    hearing.set_output(None);
    call.leave();
    discord.leave(guild);
    result
}

/// Opens the chosen speakers, or the system default if they're gone.
fn open_discord_speakers(
    state: &State,
    played: &std::sync::mpsc::SyncSender<Vec<i16>>,
    hearing: &Hearing,
) -> Result<Speakers> {
    let (speakers, output) = match discord_audio::speakers(state.speaker.as_deref(), played.clone())
    {
        Ok(opened) => opened,
        Err(error) if state.speaker.is_some() => {
            tracing::warn!(%error, "using the default speakers");
            discord_audio::speakers(None, played.clone())?
        }
        Err(error) => return Err(error),
    };
    hearing.set_output(Some(output));
    Ok(speakers)
}

/// Opens the chosen microphone, or the system default if it's gone.
fn open_discord_microphone(
    state: &State,
    reverse: &discord_audio::Reverse,
    microphone: &talk_discord::voice::Microphone,
    track: &talk_discord::voice::Track,
) -> Result<Capture> {
    let open = |id: Option<&str>| {
        discord_audio::capture(id, reverse.clone(), microphone.clone(), track.clone())
    };
    match open(state.microphone.as_deref()) {
        Ok(capture) => Ok(capture),
        Err(error) if state.microphone.is_some() => {
            tracing::warn!(%error, "using the default microphone");
            open(None)
        }
        Err(error) => Err(error),
    }
}

/// Everyone in the Discord voice channel, this account first.
fn discord_participants(
    snapshot: &talk_discord::service::Snapshot,
    channel: &str,
    me: &str,
    state: &State,
    speaking: &[u64],
    talking: bool,
) -> Vec<Participant> {
    let mut participants: Vec<Participant> = snapshot
        .voice
        .get(channel)
        .into_iter()
        .flatten()
        .map(|member| {
            let local = member.user_id == me;
            Participant {
                id: member.user_id.clone(),
                user: member.name.clone(),
                local,
                muted: if local { state.muted } else { member.muted },
                speaking: if local {
                    talking && !state.muted
                } else {
                    member
                        .user_id
                        .parse()
                        .is_ok_and(|user: u64| speaking.contains(&user))
                },
                connected: true,
                problem: None,
                avatar: Some(member.avatar.clone()),
            }
        })
        .collect();
    participants.sort_by_key(|person| std::cmp::Reverse(person.local));
    participants
}
