use anyhow::Result;
use log::{debug, info};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::LlmConfig;

/// LLM処理に関するエラー
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("Ollama APIへの接続に失敗: {0}")]
    ConnectionError(String),

    #[error("応答生成に失敗: {0}")]
    GenerationError(String),
}

/// Ollama API リクエスト
#[derive(Debug, Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    system: String,
    stream: bool,
}

/// Ollama API レスポンス
#[derive(Debug, Deserialize)]
struct GenerateResponse {
    response: String,
}

/// Ollamaを使用したLLMエンジン
pub struct OllamaLlm {
    client: Client,
    endpoint: String,
    model: String,
    system_prompt: String,
}

impl OllamaLlm {
    /// 設定からOllamaLlmインスタンスを生成
    ///
    /// # Arguments
    /// * `config` - LLM設定
    ///
    /// # Returns
    /// 初期化されたOllamaLlmインスタンス
    pub fn new(config: &LlmConfig) -> Result<Self> {
        info!(
            "Ollama LLM初期化: endpoint={}, model={}",
            config.endpoint, config.model
        );

        let client = Client::new();

        Ok(Self {
            client,
            endpoint: config.endpoint.clone(),
            model: config.model.clone(),
            system_prompt: config.system_prompt.clone(),
        })
    }

    /// プロンプトに対する応答を生成
    ///
    /// # Arguments
    /// * `prompt` - ユーザーからの入力テキスト
    ///
    /// # Returns
    /// LLMからの応答テキスト
    pub fn generate(&self, prompt: &str) -> Result<String> {
        debug!("LLM応答生成開始: \"{}\"", prompt);

        let url = format!("{}/api/generate", self.endpoint);

        let request = GenerateRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            system: self.system_prompt.clone(),
            stream: false,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .map_err(|e| LlmError::ConnectionError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(LlmError::GenerationError(format!(
                "ステータスコード: {}",
                response.status()
            ))
            .into());
        }

        let result: GenerateResponse = response
            .json()
            .map_err(|e| LlmError::GenerationError(e.to_string()))?;

        let response_text = result.response.trim().to_string();
        debug!("LLM応答生成完了: \"{}\"", response_text);

        Ok(response_text)
    }

    /// Ollamaサーバーの接続確認
    pub fn health_check(&self) -> Result<bool> {
        let url = format!("{}/api/tags", self.endpoint);

        match self.client.get(&url).send() {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    /// 利用可能なモデル一覧を取得
    pub fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/api/tags", self.endpoint);

        let response = self
            .client
            .get(&url)
            .send()
            .map_err(|e| LlmError::ConnectionError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(LlmError::ConnectionError(format!(
                "ステータスコード: {}",
                response.status()
            ))
            .into());
        }

        #[derive(Deserialize)]
        struct TagsResponse {
            models: Vec<ModelInfo>,
        }

        #[derive(Deserialize)]
        struct ModelInfo {
            name: String,
        }

        let result: TagsResponse = response
            .json()
            .map_err(|e| LlmError::ConnectionError(e.to_string()))?;

        Ok(result.models.into_iter().map(|m| m.name).collect())
    }
}
