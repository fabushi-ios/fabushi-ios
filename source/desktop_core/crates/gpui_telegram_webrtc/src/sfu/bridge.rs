use crate::media::AiMediaTap;
use gpui_telegram_core::types::{ChatId, UserId};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub struct SfuAudioFrame {
    pub chat_id: ChatId,
    pub sender_id: UserId,
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Default)]
pub struct SfuBridge {
    pub room_subscribers: HashMap<ChatId, Vec<UserId>>,
    room_ai_taps: HashMap<ChatId, Arc<dyn AiMediaTap>>,
    room_audio_channels: HashMap<ChatId, broadcast::Sender<SfuAudioFrame>>,
}

impl fmt::Debug for SfuBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SfuBridge")
            .field("room_count", &self.room_subscribers.len())
            .field("ai_tap_count", &self.room_ai_taps.len())
            .finish()
    }
}

impl SfuBridge {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn join_room(&mut self, chat_id: ChatId, user_id: UserId) {
        let subs = self.room_subscribers.entry(chat_id).or_default();
        if !subs.contains(&user_id) {
            subs.push(user_id);
        }
    }

    pub fn leave_room(&mut self, chat_id: ChatId, user_id: UserId) {
        if let Some(subs) = self.room_subscribers.get_mut(&chat_id) {
            subs.retain(|&id| id != user_id);
        }
    }

    pub fn get_subscribers(&self, chat_id: ChatId) -> &[UserId] {
        self.room_subscribers
            .get(&chat_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn attach_ai_tap(&mut self, chat_id: ChatId, tap: Arc<dyn AiMediaTap>) {
        self.room_ai_taps.insert(chat_id, tap);
    }

    pub fn detach_ai_tap(&mut self, chat_id: ChatId) {
        self.room_ai_taps.remove(&chat_id);
    }

    pub fn get_ai_tap(&self, chat_id: ChatId) -> Option<Arc<dyn AiMediaTap>> {
        self.room_ai_taps.get(&chat_id).cloned()
    }

    pub fn subscribe_room_audio(&mut self, chat_id: ChatId) -> broadcast::Receiver<SfuAudioFrame> {
        let tx = self
            .room_audio_channels
            .entry(chat_id)
            .or_insert_with(|| broadcast::channel(64).0);
        tx.subscribe()
    }

    pub fn route_incoming_pcm(
        &self,
        chat_id: ChatId,
        sender_id: UserId,
        pcm: &[i16],
        sample_rate: u32,
        channels: u16,
    ) {
        if let Some(tap) = self.room_ai_taps.get(&chat_id) {
            tap.on_incoming_pcm_frame(pcm, sample_rate, channels);
        }
        if let Some(tx) = self.room_audio_channels.get(&chat_id) {
            let _ = tx.send(SfuAudioFrame {
                chat_id,
                sender_id,
                pcm: pcm.to_vec(),
                sample_rate,
                channels,
            });
        }
    }

    pub fn route_outgoing_tts_pcm(
        &self,
        chat_id: ChatId,
        bot_user_id: UserId,
        pcm: &[i16],
        sample_rate: u32,
        channels: u16,
    ) {
        if let Some(tap) = self.room_ai_taps.get(&chat_id) {
            tap.on_outgoing_pcm_frame(pcm, sample_rate, channels);
        }
        if let Some(tx) = self.room_audio_channels.get(&chat_id) {
            let _ = tx.send(SfuAudioFrame {
                chat_id,
                sender_id: bot_user_id,
                pcm: pcm.to_vec(),
                sample_rate,
                channels,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::DefaultAiMediaTap;

    #[test]
    fn test_sfu_bridge_ai_tap_routing_and_barge_in() {
        let mut sfu = SfuBridge::new();
        let chat = ChatId(500);
        let user = UserId(1);
        sfu.join_room(chat, user);
        let tap = Arc::new(DefaultAiMediaTap::new(48_000, 1, 0.05).with_consecutive_frames(2));
        sfu.attach_ai_tap(chat, tap.clone());
        let mut rx = sfu.subscribe_room_audio(chat);

        let speech = vec![12_000i16; 960];
        sfu.route_incoming_pcm(chat, user, &speech, 48_000, 1);
        sfu.route_incoming_pcm(chat, user, &speech, 48_000, 1);
        assert!(tap.trigger_vad_barge_in());
        let frame = rx.try_recv().unwrap();
        assert_eq!(frame.chat_id, chat);
        assert_eq!(frame.pcm.len(), 960);
    }
}
