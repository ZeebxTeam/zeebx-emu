//! ISound and ISoundPlayer: low-level audio output.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Turns on sound output. Without it the emulator runs the same, muted.
    pub fn set_audio(&mut self, mixer: Option<crate::audio::Mixer>) {
        self.audio = mixer;
    }

    pub(super) fn sound_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Sound.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.sounds.remove(&this);
                }
                remaining
            }
            // void RegisterNotify(ISound *, PFNSOUNDSTATUS pfn, const void *pUser)
            "RegisterNotify" => {
                self.sounds.entry(this).or_default().notify = Callback {
                    function: a1,
                    context: a2,
                };
                SUCCESS
            }
            // int Set(ISound *, const AEESoundInfo *) — five `int8`s: eDevice, eMethod, eAPath,
            // eEarMuteCtl, eMicMuteCtl.
            "Set" => {
                if a1 != 0 {
                    let mut info = [0u8; 5];
                    self.cpu.read_mem(a1, &mut info)?;
                    self.sounds.entry(this).or_default().info = info;
                }
                SUCCESS
            }
            "Get" => {
                if a1 != 0 {
                    let info = self.sounds.entry(this).or_default().info;
                    self.cpu.write_mem(a1, &info)?;
                }
                SUCCESS
            }
            "SetDevice" => SUCCESS,
            // Playing is instant and silent, but the game usually waits for the completion
            // callback before releasing the audio resource.
            "PlayTone" | "PlayToneList" | "PlayFreqTone" => {
                self.finish_sound(this, AEE_SOUND_STATUS_CB, AEE_SOUND_PLAY_DONE);
                SUCCESS
            }
            "Vibrate" => {
                self.finish_sound(this, AEE_SOUND_STATUS_CB, AEE_SOUND_PLAY_DONE);
                SUCCESS
            }
            "StopTone" | "StopVibrate" => SUCCESS,
            "SetVolume" => {
                self.sounds.entry(this).or_default().volume = (a1 as u16).min(AEE_MAX_VOLUME);
                SUCCESS
            }
            // `GetVolume` returns `void`: the value reaches the game through the callback, with
            // `AEE_SOUND_VOLUME_CB` and the volume in `dwParam`.
            "GetVolume" => {
                let volume = self.sounds.entry(this).or_default().volume;
                self.finish_sound_with(this, AEE_SOUND_VOLUME_CB, AEE_SOUND_SUCCESS, volume as u32);
                SUCCESS
            }
            // There is no contention for the audio resource in an emulator that runs one game.
            "GetResourceCtl" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, 0)?;
                }
                ECLASSNOTSUPPORT
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// `ISoundPlayer`: only what a game has needed so far. The other slots keep their SDK names
    /// and show up under "missing APIs" when called.
    ///
    /// Devil May Cry: Dante x Virgil creates one next to its `ISound`, registers a callback and
    /// asks for the volume. It never plays through it — its music and effects go through
    /// `IMedia` — but its sound manager only starts once the class exists: refused, the game
    /// stayed mute. The state is an `ISound`'s (callback and volume), so it lives in the same map
    /// and goes into the save state with it.
    pub(super) fn sound_player_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SoundPlayer.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.sounds.remove(&this);
                }
                remaining
            }
            // void RegisterNotify(ISoundPlayer *, PFNSOUNDPLAYERSTATUS pfn, void *pUser)
            "RegisterNotify" => {
                self.sounds.entry(this).or_default().notify = Callback {
                    function: a1,
                    context: a2,
                };
                SUCCESS
            }
            // void SetVolume(ISoundPlayer *, uint16 wVolume)
            "SetVolume" => {
                self.sounds.entry(this).or_default().volume = (a1 as u16).min(AEE_MAX_VOLUME);
                SUCCESS
            }
            // void GetVolume(ISoundPlayer *) — the answer is the callback with
            // `AEE_SOUNDPLAYER_VOLUME_CB` and `dwParam` pointing at an `AEESoundPlayerCmdData`,
            // whose `wVolume` is its first field. Unlike `ISound`, the volume is **not** the
            // parameter itself: the game reads it with a `ldrh` from `dwParam`.
            //
            // The struct lives inside the object's own slot: an object takes 64 bytes of the
            // object region and uses only the first word, so `this + 8` is ours, needs no
            // allocation and travels with the save state.
            "GetVolume" => {
                let volume = self.sounds.entry(this).or_default().volume;
                let data = this + 8;
                self.cpu.write_mem(data, &volume.to_le_bytes())?;
                self.finish_sound_with(this, AEE_SOUNDPLAYER_VOLUME_CB, AEE_SOUNDPLAYER_SUCCESS, data);
                SUCCESS
            }
            // int SetInfo(ISoundPlayer *, AEESoundPlayerInfo *pInfo) — `{int8 eInput, void *pData,
            // uint32 dwSize}`, answered at once. Ultimate Chess 3D hands it a buffer at
            // `EVT_APP_START` and asks for the volume only when this succeeds; nothing has played
            // through the source yet, so it is only kept, for `GetInfo`.
            //
            // It is kept in the object's own slot, after the `GetVolume` data (see above): `this +
            // 16`, twelve bytes, already in the save state. For `SDT_FILE` the SDK copies the name;
            // the pointer is enough while nothing reads the file.
            "SetInfo" => {
                let source = this + SOUND_PLAYER_SOURCE;
                if a1 == 0 {
                    self.cpu.write_mem(source, &[0u8; 12])?;
                    return Ok(Some(SUCCESS));
                }
                let mut info = [0u8; 12];
                self.cpu.read_mem(a1, &mut info)?;
                let size = u32::from_le_bytes(info[8..12].try_into().unwrap());
                match info[0] {
                    SDT_FILE | SDT_BUFFER | SDT_VOICEPROMPT if size != 0 => {
                        self.cpu.write_mem(source, &info)?;
                        SUCCESS
                    }
                    SDT_FILE | SDT_BUFFER | SDT_VOICEPROMPT => EBADPARM,
                    _ => EUNSUPPORTED,
                }
            }
            // int GetInfo(ISoundPlayer *, AEESoundPlayerInfo *pInfo) — the source `SetInfo` kept.
            "GetInfo" => {
                if a1 == 0 {
                    return Ok(Some(EBADPARM));
                }
                let mut info = [0u8; 12];
                self.cpu.read_mem(this + SOUND_PLAYER_SOURCE, &mut info)?;
                self.cpu.write_mem(a1, &info)?;
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Queues the `ISound` callback with no extra parameter.
    pub(super) fn finish_sound(&mut self, sound: u32, kind: u32, status: u32) {
        self.finish_sound_with(sound, kind, status, 0);
    }

    /// Queues the sound's `PFNSOUNDSTATUS(pUser, eCBType, eSPStatus, dwParam)`.
    pub(super) fn finish_sound_with(&mut self, sound: u32, kind: u32, status: u32, param: u32) {
        let Some(state) = self.sounds.get(&sound) else {
            return;
        };
        if state.notify.function == 0 {
            return;
        }
        let notify = state.notify;
        self.pending_calls.push(GuestCall {
            function: notify.function,
            args: [notify.context, kind, status, param],
        });
    }
}
