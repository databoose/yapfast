//! Joining and switching calls, and answering them, and the sounds they make.

use super::*;
use crate::sounds::Sound;

/// The sound a change in the call makes: this account joining or leaving it,
/// or, while it's there, someone else connecting or going. People arriving
/// together make one sound. Reconnecting makes none, and nor do those a DM
/// call is still ringing, until they answer.
pub(super) fn call_sounds(before: &voice::State, after: &voice::State) -> Vec<Effect> {
    let here = |state: &voice::State| matches!(state.phase, Phase::Connected | Phase::Reconnecting);
    let others = |state: &voice::State| -> HashSet<String> {
        state
            .participants
            .iter()
            .filter(|person| !person.local && person.connected)
            .map(|person| person.id.clone())
            .collect()
    };
    let sound = if after.phase == Phase::Connected && (!here(before) || before.room != after.room) {
        Some(Sound::Join)
    } else if here(before) && !here(after) {
        Some(Sound::Leave)
    } else if before.phase == Phase::Connected && after.phase == Phase::Connected {
        let (before, after) = (others(before), others(after));
        if !after.is_subset(&before) {
            Some(Sound::Join)
        } else if !before.is_subset(&after) {
            Some(Sound::Leave)
        } else {
            None
        }
    } else {
        None
    };
    sound.map(Effect::Sound).into_iter().collect()
}

impl Application {
    pub(super) fn request_join_voice(&mut self, id: String) -> Vec<Effect> {
        if !self.state.can_join_voice(&id) {
            return vec![];
        }
        if self.state.voice.active() {
            if self.state.voice.room.as_ref() != Some(&id) {
                self.state.switch_voice = Some(id);
            }
            vec![]
        } else {
            self.voice_command(voice::Command::Join(id))
        }
    }

    pub(super) fn accept_call(&mut self, id: String) -> Vec<Effect> {
        if !self.state.can_join_voice(&id) {
            return vec![];
        }
        let mut effects = self.dispatch(Action::Open(id.clone()));
        if self.state.voice.room.as_ref() != Some(&id) || !self.state.voice.active() {
            if self.state.voice.active() {
                effects.extend(self.voice_command(voice::Command::Leave));
            }
            effects.extend(self.voice_command(voice::Command::Join(id)));
        }
        effects
    }

    pub(super) fn confirm_voice_switch(&mut self) -> Vec<Effect> {
        let Some(id) = self.state.switch_voice.take() else {
            return vec![];
        };
        if !self.state.can_join_voice(&id) {
            return vec![];
        }
        let mut effects = self.voice_command(voice::Command::Leave);
        effects.extend(self.voice_command(voice::Command::Join(id)));
        effects
    }

    /// A call command, with the sound of muting or deafening in a call.
    pub(super) fn voice_command(&mut self, command: voice::Command) -> Vec<Effect> {
        let connected = self.state.voice.phase == Phase::Connected;
        let sound = match &command {
            _ if !connected => None,
            voice::Command::Deafen(true) => Some(Sound::Deafen),
            // Unmuting undeafens too.
            voice::Command::Deafen(false) | voice::Command::Mute(false)
                if self.state.voice.deafened =>
            {
                Some(Sound::Undeafen)
            }
            voice::Command::Mute(true) => Some(Sound::Mute),
            voice::Command::Mute(false) => Some(Sound::Unmute),
            _ => None,
        };
        let mut effects = self.voice_command_quietly(command);
        effects.extend(sound.map(Effect::Sound));
        effects
    }

    pub(super) fn voice_command_quietly(&mut self, command: voice::Command) -> Vec<Effect> {
        if matches!(command, voice::Command::Leave) {
            self.state.switch_voice = None;
        }
        if !self.state.preview {
            return vec![Effect::Backend(Command::Voice(command))];
        }
        let before = self.state.voice.clone();
        match command {
            voice::Command::Join(id) => {
                if let Some(room) = self
                    .state
                    .rooms
                    .iter()
                    .find(|room| room.id == id && room.voice)
                {
                    self.state.voice = voice::State {
                        phase: Phase::Connected,
                        room: Some(id),
                        name: room.name.clone(),
                        participants: vec![voice::Participant {
                            id: "preview-you".into(),
                            user: "You".into(),
                            person: "preview-you".into(),
                            local: true,
                            muted: true,
                            deafened: false,
                            speaking: false,
                            connected: true,
                            problem: None,
                            avatar: None,
                            streaming: false,
                            video: None,
                        }],
                        can_film: true,
                        ..self.preview_voice()
                    };
                }
            }
            voice::Command::Leave => {
                let filming = self.state.voice.camera_on;
                self.state.voice = self.preview_voice();
                if filming {
                    let mut effects = call_sounds(&before, &self.state.voice);
                    effects.push(Effect::PreviewCamera(None));
                    return effects;
                }
            }
            voice::Command::Video(on) if self.state.voice.active() => {
                let feed = on.then(crate::picture::Feed::new);
                for person in &mut self.state.voice.participants {
                    if person.local {
                        person.video = feed.clone();
                    }
                }
                self.state.voice.camera_on = on;
                let camera = self.state.voice.camera.clone();
                return vec![Effect::PreviewCamera(feed.map(|feed| (camera, feed)))];
            }
            voice::Command::Devices => return vec![Effect::ListDevices],
            voice::Command::Microphone(id) => self.state.voice.microphone = Some(id),
            voice::Command::Speaker(id) => self.state.voice.speaker = Some(id),
            voice::Command::Camera(id) => self.state.voice.camera = Some(id),
            voice::Command::Mute(muted) => {
                if !muted {
                    self.state.voice.deafened = false;
                }
                self.state.voice.muted = muted;
            }
            voice::Command::Deafen(deafened) => {
                self.state.voice.deafened = deafened;
                if deafened {
                    self.state.voice.muted = true;
                }
            }
            _ => {}
        }
        for participant in &mut self.state.voice.participants {
            participant.muted = self.state.voice.muted;
        }
        call_sounds(&before, &self.state.voice)
    }
}
