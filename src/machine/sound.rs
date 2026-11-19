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
