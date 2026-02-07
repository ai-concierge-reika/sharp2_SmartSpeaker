use anyhow::Result;
use log::{debug, info};
use reqwest::blocking::Client;
use serde_json::Value;
use thiserror::Error;

use crate::config::TtsConfig;

/// TTS処理に関するエラー
#[derive(Debug, Error)]
pub enum TtsError {
    #[error("VOICEVOX APIへの接続に失敗: {0}")]
    ConnectionError(String),

    #[error("音声クエリの作成に失敗: {0}")]
    AudioQueryError(String),

    #[error("音声合成に失敗: {0}")]
    SynthesisError(String),
}

/// VOICEVOXを使用した音声合成エンジン
pub struct VoicevoxTts {
    client: Client,
    endpoint: String,
    speaker_id: i32,
    speed: f32,
}

impl VoicevoxTts {
    /// 設定からVoicevoxTtsインスタンスを生成
    ///
    /// # Arguments
    /// * `config` - TTS設定
    ///
    /// # Returns
    /// 初期化されたVoicevoxTtsインスタンス
    pub fn new(config: &TtsConfig) -> Result<Self> {
        info!("VOICEVOX TTS初期化: endpoint={}, speaker_id={}", config.endpoint, config.speaker_id);

        let client = Client::new();

        Ok(Self {
            client,
            endpoint: config.endpoint.clone(),
            speaker_id: config.speaker_id,
            speed: config.speed,
        })
    }

    /// テキストを音声データに変換
    ///
    /// # Arguments
    /// * `text` - 合成するテキスト
    ///
    /// # Returns
    /// WAV形式の音声データ（バイト列）
    pub fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        debug!("音声合成開始: \"{}\"", text);

        // 1. audio_queryを作成
        let query = self.create_audio_query(text)?;

        // 2. 音声合成を実行
        let audio = self.synthesis(&query)?;

        debug!("音声合成完了: {} bytes", audio.len());
        Ok(audio)
    }

    /// 音声合成用クエリを作成
    fn create_audio_query(&self, text: &str) -> Result<Value> {
        let url = format!(
            "{}/audio_query?text={}&speaker={}",
            self.endpoint,
            urlencoding::encode(text),
            self.speaker_id
        );

        debug!("audio_query API呼び出し: {}", url);

        let response = self
            .client
            .post(&url)
            .send()
            .map_err(|e| TtsError::ConnectionError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(TtsError::AudioQueryError(format!(
                "ステータスコード: {}",
                response.status()
            ))
            .into());
        }

        let mut query: Value = response
            .json()
            .map_err(|e| TtsError::AudioQueryError(e.to_string()))?;

        // 話速を設定
        if let Some(obj) = query.as_object_mut() {
            obj.insert("speedScale".to_string(), Value::from(self.speed));
        }

        Ok(query)
    }

    /// 音声合成を実行
    fn synthesis(&self, query: &Value) -> Result<Vec<u8>> {
        let url = format!("{}/synthesis?speaker={}", self.endpoint, self.speaker_id);

        debug!("synthesis API呼び出し");

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(query)
            .send()
            .map_err(|e| TtsError::ConnectionError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(TtsError::SynthesisError(format!(
                "ステータスコード: {}",
                response.status()
            ))
            .into());
        }

        let audio = response
            .bytes()
            .map_err(|e| TtsError::SynthesisError(e.to_string()))?
            .to_vec();

        Ok(audio)
    }

    /// VOICEVOXサーバーの接続確認
    pub fn health_check(&self) -> Result<bool> {
        let url = format!("{}/version", self.endpoint);

        match self.client.get(&url).send() {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}
