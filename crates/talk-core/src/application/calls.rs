//! Joining and switching calls, and answering them.

use super::*;

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

    pub(super) fn voice_command(&mut self, command: voice::Command) -> Vec<Effect> {
        if matches!(command, voice::Command::Leave) {
            self.state.switch_voice = None;
        }
        if !self.state.preview {
            return vec![Effect::Backend(Command::Voice(command))];
        }
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
                        }],
                        ..self.preview_voice()
                    };
                }
            }
            voice::Command::Leave => self.state.voice = self.preview_voice(),
            voice::Command::Devices => return vec![Effect::ListDevices],
            voice::Command::Microphone(id) => self.state.voice.microphone = Some(id),
            voice::Command::Speaker(id) => self.state.voice.speaker = Some(id),
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
        vec![]
    }
}
