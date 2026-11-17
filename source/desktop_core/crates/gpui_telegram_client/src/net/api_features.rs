use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::{decode_json_response, extract_api_error, percent_encode_rfc3986};
use gpui_telegram_proto::contracts::{
    CallInviteRequest, CallRecordDto, CallSignalRequest, ChannelStatisticsDto, GifResultDto,
    MediaUploadCompleteResponse, SearchResponse, StarGiftRequest, StickerSetDto, StoryDto,
    WalletBalanceDto, WalletTransactionDto,
};
use reqwest::multipart::{Form, Part};
use reqwest::Method;
use serde::Serialize;

#[derive(Serialize)]
struct CreateStoryBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    media_id: Option<&'a str>,
    media_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    caption: Option<&'a str>,
    privacy: &'a str,
}

#[derive(Serialize)]
struct WalletTopupBody {
    amount_stars: i64,
}

#[derive(Serialize)]
struct StickerInstallBody {
    installed: bool,
}

#[derive(Serialize)]
struct CallIdBody<'a> {
    call_id: &'a str,
}

#[derive(Serialize)]
struct RateCallBody<'a> {
    rating: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback: Option<&'a str>,
}

impl HttpClient {
    pub async fn upload_media_multipart(
        &self,
        token: &str,
        filename: &str,
        mime_type: &str,
        data: Vec<u8>,
    ) -> Result<MediaUploadCompleteResponse, HttpClientError> {
        let client = self.client.clone();
        let url = self.endpoint_url("/api/v1/media/upload");
        let bearer = token.to_string();
        let name = filename.to_string();
        let mime = mime_type.to_string();
        self.run_on_tokio(async move {
            let part = Part::bytes(data).file_name(name).mime_str(&mime)?;
            let form = Form::new().part("file", part);
            let resp = client
                .post(&url)
                .bearer_auth(&bearer)
                .multipart(form)
                .send()
                .await?;
            let status = resp.status();
            let text = resp.text().await?;
            decode_json_response(status, text)
        })
        .await
    }

    pub async fn download_media_range(
        &self,
        token: &str,
        file_id: &str,
        start: u64,
        end_inclusive: Option<u64>,
    ) -> Result<(u16, Vec<u8>, Option<String>), HttpClientError> {
        let path = format!("/api/v1/media/{}", percent_encode_rfc3986(file_id));
        let range = match end_inclusive {
            Some(end) => format!("bytes={}-{}", start, end),
            None => format!("bytes={}-", start),
        };
        self.request_bytes(Method::GET, &path, Some(token), Some(&range))
            .await
    }

    pub async fn list_sticker_sets(
        &self,
        token: &str,
        query: Option<&str>,
    ) -> Result<Vec<StickerSetDto>, HttpClientError> {
        let path = match query.filter(|q| !q.trim().is_empty()) {
            Some(q) => format!("/api/v1/stickers/sets?q={}", percent_encode_rfc3986(q)),
            None => "/api/v1/stickers/sets".to_string(),
        };
        self.request_vec_or_field(Method::GET, &path, Some(token), &["sets", "sticker_sets", "items"])
            .await
    }

    pub async fn install_sticker_set(
        &self,
        token: &str,
        set_id: &str,
        installed: bool,
    ) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/stickers/sets/{}", percent_encode_rfc3986(set_id));
        let body = StickerInstallBody { installed };
        self.request_empty(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn search_gifs(
        &self,
        token: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GifResultDto>, HttpClientError> {
        let path = format!(
            "/api/v1/gifs/search?q={}&limit={}",
            percent_encode_rfc3986(query),
            limit.clamp(1, 50)
        );
        self.request_vec_or_field(Method::GET, &path, Some(token), &["gifs", "results", "items"])
            .await
    }

    pub async fn list_stories(&self, token: &str) -> Result<Vec<StoryDto>, HttpClientError> {
        self.request_vec_or_field(Method::GET, "/api/v1/stories", Some(token), &["stories", "items"])
            .await
    }

    pub async fn create_story(
        &self,
        token: &str,
        media_id: Option<&str>,
        media_type: &str,
        caption: Option<&str>,
        privacy: &str,
    ) -> Result<StoryDto, HttpClientError> {
        let body = CreateStoryBody {
            media_id,
            media_type,
            caption,
            privacy,
        };
        self.request_json(Method::POST, "/api/v1/stories", Some(token), Some(&body))
            .await
    }

    pub async fn delete_story(&self, token: &str, story_id: &str) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/stories/{}", percent_encode_rfc3986(story_id));
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_calls(&self, token: &str) -> Result<Vec<CallRecordDto>, HttpClientError> {
        self.request_vec_or_field(Method::GET, "/api/v1/calls", Some(token), &["calls", "items"])
            .await
    }

    pub async fn call_invite(
        &self,
        token: &str,
        req: &CallInviteRequest,
    ) -> Result<CallRecordDto, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/calls/invite", Some(token), Some(req))
            .await
    }

    pub async fn call_accept(
        &self,
        token: &str,
        call_id: &str,
    ) -> Result<CallRecordDto, HttpClientError> {
        let body = CallIdBody { call_id };
        self.request_json(Method::POST, "/api/v1/calls/accept", Some(token), Some(&body))
            .await
    }

    pub async fn call_decline(&self, token: &str, call_id: &str) -> Result<(), HttpClientError> {
        let body = CallIdBody { call_id };
        self.request_empty(Method::POST, "/api/v1/calls/decline", Some(token), Some(&body))
            .await
    }

    pub async fn call_hangup(&self, token: &str, call_id: &str) -> Result<(), HttpClientError> {
        let body = CallIdBody { call_id };
        self.request_empty(Method::POST, "/api/v1/calls/hangup", Some(token), Some(&body))
            .await
    }

    pub async fn rate_call(
        &self,
        token: &str,
        call_id: &str,
        rating: u8,
        feedback: Option<&str>,
    ) -> Result<CallRecordDto, HttpClientError> {
        let path = format!("/api/v1/calls/{}/rate", percent_encode_rfc3986(call_id));
        let body = RateCallBody { rating, feedback };
        self.request_json(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn call_signal(
        &self,
        token: &str,
        req: &CallSignalRequest,
    ) -> Result<(), HttpClientError> {
        self.request_empty(Method::POST, "/api/v1/calls/signal", Some(token), Some(req))
            .await
    }

    pub async fn get_turn_credentials(
        &self,
        token: &str,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(
            Method::GET,
            "/api/v1/webrtc/turn_credentials",
            Some(token),
            None::<&()>,
        )
        .await
    }

    pub async fn search_messages(
        &self,
        token: &str,
        query: &str,
        filter: Option<&str>,
        chat_id: Option<i64>,
        limit: usize,
    ) -> Result<SearchResponse, HttpClientError> {
        let mut path = format!(
            "/api/v1/search/messages?q={}&limit={}",
            percent_encode_rfc3986(query),
            limit.clamp(1, 100)
        );
        if let Some(f) = filter.filter(|s| !s.is_empty()) {
            path.push_str("&filter=");
            path.push_str(&percent_encode_rfc3986(f));
        }
        if let Some(cid) = chat_id {
            path.push_str(&format!("&chat_id={}", cid));
        }
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn get_channel_statistics(
        &self,
        token: &str,
        chat_id: i64,
    ) -> Result<ChannelStatisticsDto, HttpClientError> {
        let path = format!("/api/v1/chats/{}/statistics", chat_id);
        self.request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn export_chat_history(
        &self,
        token: &str,
        chat_id: i64,
        format: Option<&str>,
    ) -> Result<String, HttpClientError> {
        let path = match format {
            Some(fmt) => format!(
                "/api/v1/chats/{}/export?format={}",
                chat_id,
                percent_encode_rfc3986(fmt)
            ),
            None => format!("/api/v1/chats/{}/export", chat_id),
        };
        let client = self.client.clone();
        let url = self.endpoint_url(&path);
        let bearer = token.to_string();
        self.run_on_tokio(async move {
            let resp = client.get(&url).bearer_auth(&bearer).send().await?;
            let status = resp.status();
            let text = resp.text().await?;
            if !status.is_success() {
                return Err(extract_api_error(status, &text));
            }
            Ok(text)
        })
        .await
    }

    pub async fn wallet_balance(&self, token: &str) -> Result<WalletBalanceDto, HttpClientError> {
        self.request_json(Method::GET, "/api/v1/wallet/balance", Some(token), None::<&()>)
            .await
    }

    pub async fn wallet_transactions(
        &self,
        token: &str,
    ) -> Result<Vec<WalletTransactionDto>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/wallet/transactions",
            Some(token),
            &["transactions", "items"],
        )
        .await
    }

    pub async fn wallet_topup(
        &self,
        token: &str,
        amount_stars: i64,
    ) -> Result<WalletBalanceDto, HttpClientError> {
        let body = WalletTopupBody { amount_stars };
        let val: serde_json::Value = self
            .request_json(Method::POST, "/api/v1/wallet/topup", Some(token), Some(&body))
            .await?;
        let target = val.get("balance").cloned().unwrap_or(val);
        Ok(serde_json::from_value(target)?)
    }

    pub async fn wallet_gift_stars(
        &self,
        token: &str,
        req: &StarGiftRequest,
    ) -> Result<WalletBalanceDto, HttpClientError> {
        let val: serde_json::Value = self
            .request_json(Method::POST, "/api/v1/wallet/gift", Some(token), Some(req))
            .await?;
        let target = val.get("balance").cloned().unwrap_or(val);
        Ok(serde_json::from_value(target)?)
    }
}
