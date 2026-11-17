use crate::cache::sqlite_cache::sha256_hex;
use crate::client::TelegramClient;
use crate::net::{
    CallInviteRequest, CallRecordDto, CallSignalRequest, ChannelStatisticsDto, GifResultDto,
    HttpClientError, MediaUploadCompleteResponse, SearchResponse, StarGiftRequest, StickerSetDto,
    StoryDto, WalletBalanceDto, WalletTransactionDto,
};
use gpui_telegram_proto::ws::WsClientFrame;
use std::path::PathBuf;

const MULTIPART_UPLOAD_THRESHOLD_BYTES: usize = 256 * 1024;

impl TelegramClient {
    pub fn local_media_path(&self, file_id: &str) -> Option<PathBuf> {
        self.local_store.media_cas_path(file_id)
    }

    pub async fn upload_file_resumable(
        &self,
        file_path: impl AsRef<std::path::Path>,
        mime_type: &str,
        chat_id: Option<i64>,
        transfer_id: Option<&str>,
    ) -> gpui_telegram_core::error::Result<MediaUploadCompleteResponse> {
        self.chunked_transfers
            .start_or_resume_upload(file_path.as_ref(), mime_type, chat_id, transfer_id)
            .await
    }

    pub async fn upload_media(
        &self,
        filename: &str,
        mime_type: &str,
        data: Vec<u8>,
    ) -> std::result::Result<MediaUploadCompleteResponse, HttpClientError> {
        let token = self.require_token()?;
        let res = if data.len() <= MULTIPART_UPLOAD_THRESHOLD_BYTES {
            self.http_client
                .upload_media_multipart(&token, filename, mime_type, data.clone())
                .await?
        } else {
            let file_id = self
                .http_client
                .upload_media(&token, filename, &data)
                .await?;
            MediaUploadCompleteResponse {
                file_id,
                sha256: sha256_hex(&data),
                size_bytes: data.len() as u64,
                mime_type: mime_type.to_string(),
                filename: Some(filename.to_string()),
                width: None,
                height: None,
                duration_seconds: None,
                waveform_b64: None,
                waveform_bars: None,
                thumbnail_data_url: None,
            }
        };
        let _ = self
            .local_store
            .put_media_cas(&res.file_id, &data, Some(&res.mime_type));
        Ok(res)
    }

    pub async fn get_media_bytes(
        &self,
        file_id: &str,
    ) -> std::result::Result<Vec<u8>, HttpClientError> {
        if let Some(cached) = self.local_store.get_media_cas(file_id) {
            return Ok(cached);
        }
        let token = self.require_token()?;
        let bytes = self.http_client.download_media(&token, file_id).await?;
        let _ = self.local_store.put_media_cas(file_id, &bytes, None);
        Ok(bytes)
    }

    pub async fn download_media_range(
        &self,
        file_id: &str,
        start: u64,
        end_inclusive: Option<u64>,
    ) -> std::result::Result<(u16, Vec<u8>, Option<String>), HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .download_media_range(&token, file_id, start, end_inclusive)
            .await
    }

    pub async fn list_sticker_sets(
        &self,
        query: Option<&str>,
    ) -> std::result::Result<Vec<StickerSetDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_sticker_sets(&token, query).await
    }

    pub async fn install_sticker_set(
        &self,
        set_id: &str,
        installed: bool,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .install_sticker_set(&token, set_id, installed)
            .await
    }

    pub async fn search_gifs(
        &self,
        query: &str,
        limit: usize,
    ) -> std::result::Result<Vec<GifResultDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.search_gifs(&token, query, limit).await
    }

    pub async fn list_stories(&self) -> std::result::Result<Vec<StoryDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_stories(&token).await
    }

    pub async fn create_story(
        &self,
        media_id: Option<&str>,
        media_type: &str,
        caption: Option<&str>,
        privacy: &str,
    ) -> std::result::Result<StoryDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .create_story(&token, media_id, media_type, caption, privacy)
            .await
    }

    pub async fn delete_story(&self, story_id: &str) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.delete_story(&token, story_id).await
    }

    pub async fn list_calls(&self) -> std::result::Result<Vec<CallRecordDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_calls(&token).await
    }

    pub async fn call_invite(
        &self,
        req: &CallInviteRequest,
    ) -> std::result::Result<CallRecordDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.call_invite(&token, req).await
    }

    pub async fn call_accept(&self, call_id: &str) -> std::result::Result<CallRecordDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.call_accept(&token, call_id).await
    }

    pub async fn call_decline(&self, call_id: &str) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.call_decline(&token, call_id).await
    }

    pub async fn call_hangup(&self, call_id: &str) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.call_hangup(&token, call_id).await
    }

    pub async fn rate_call(
        &self,
        call_id: &str,
        rating: u8,
    ) -> std::result::Result<CallRecordDto, HttpClientError> {
        self.rate_call_with_feedback(call_id, rating, None).await
    }

    pub async fn rate_call_with_feedback(
        &self,
        call_id: &str,
        rating: u8,
        feedback: Option<&str>,
    ) -> std::result::Result<CallRecordDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .rate_call(&token, call_id, rating, feedback)
            .await
    }

    pub async fn send_call_signal(
        &self,
        req: CallSignalRequest,
        target_user_id: Option<u64>,
    ) -> std::result::Result<(), HttpClientError> {
        if self.is_connected() {
            let _ = self.send_ws_frame(WsClientFrame::CallSignal {
                call_id: req.call_id.clone(),
                signal_type: req.signal_type.clone(),
                target_user_id,
                sdp: req.sdp.clone(),
                candidate: req.candidate.clone(),
                sdp_mid: req.sdp_mid.clone(),
                sdp_mline_index: req.sdp_mline_index,
            });
        }
        let token = self.require_token()?;
        self.http_client.call_signal(&token, &req).await
    }

    pub async fn get_turn_credentials(&self) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_turn_credentials(&token).await
    }

    pub async fn search_messages(
        &self,
        query: &str,
        filter: Option<&str>,
        chat_id: Option<i64>,
        limit: usize,
    ) -> std::result::Result<SearchResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .search_messages(&token, query, filter, chat_id, limit)
            .await
    }

    pub async fn get_channel_statistics(
        &self,
        chat_id: i64,
    ) -> std::result::Result<ChannelStatisticsDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_channel_statistics(&token, chat_id).await
    }

    pub async fn export_chat_history(
        &self,
        chat_id: i64,
        format: Option<&str>,
    ) -> std::result::Result<String, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .export_chat_history(&token, chat_id, format)
            .await
    }

    pub async fn wallet_balance(&self) -> std::result::Result<WalletBalanceDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.wallet_balance(&token).await
    }

    pub async fn wallet_transactions(
        &self,
    ) -> std::result::Result<Vec<WalletTransactionDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.wallet_transactions(&token).await
    }

    pub async fn wallet_topup(
        &self,
        amount_stars: i64,
    ) -> std::result::Result<WalletBalanceDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.wallet_topup(&token, amount_stars).await
    }

    pub async fn wallet_gift_stars(
        &self,
        req: &StarGiftRequest,
    ) -> std::result::Result<WalletBalanceDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.wallet_gift_stars(&token, req).await
    }
}
