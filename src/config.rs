use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

/// アプリケーション全体の設定
#[derive(Debug, Deserialize)]
pub struct Config {
    pub audio: AudioConfig,
    pub wakeword: WakewordConfig,
    pub stt: SttConfig,
    pub llm: LlmConfig,
    pub tts: TtsConfig,
}

/// ウェイクワード検出の設定（Rustpotter）
#[derive(Debug, Deserialize)]
pub struct WakewordConfig {
    /// ウェイクワードファイルのパス（.rpwファイル）
    pub wakeword_path: String,
    /// 検出閾値（0.0〜1.0、デフォルト0.35）
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    /// 平均スコア閾値（0.0〜1.0、デフォルト0.15）
    #[serde(default = "default_avg_threshold")]
    pub avg_threshold: f32,
    /// 連続検出回数（単発の誤検出を防ぐ、デフォルト3）
    #[serde(default = "default_min_scores")]
    pub min_scores: usize,
}

fn default_threshold() -> f32 {
    0.35
}

fn default_avg_threshold() -> f32 {
    0.15
}

fn default_min_scores() -> usize {
    3
}

/// オーディオ入出力の設定
#[derive(Debug, Deserialize)]
pub struct AudioConfig {
    /// サンプルレート（通常16000Hz）
    pub sample_rate: u32,
    /// 録音最大時間（秒）
    pub max_record_seconds: f32,
    /// 無音検出閾値（0.0〜1.0）
    pub silence_threshold: f32,
    /// 無音継続時間で録音終了（秒）
    pub silence_duration: f32,
    /// 入力ゲイン（1.0 = 変更なし、デフォルト1.0）
    #[serde(default = "default_input_gain")]
    pub input_gain: f32,
    /// RMS平滑化係数（0.0〜1.0、デフォルト0.1）
    /// 小さいほど滑らかに追従し、瞬間的なノイズを無視
    #[serde(default = "default_smoothing_alpha")]
    pub smoothing_alpha: f32,
    /// 相対閾値の乗数（デフォルト3.0）
    /// 発話はノイズフロアの何倍で検出するか
    #[serde(default = "default_relative_threshold_multiplier")]
    pub relative_threshold_multiplier: f32,
    /// ノイズフロアキャリブレーション期間（秒、デフォルト0.5）
    #[serde(default = "default_calibration_duration")]
    pub calibration_duration: f32,
    /// 無音判定のデバウンスフレーム数（デフォルト3）
    /// 連続した無音フレームがこの回数以上続いたら無音としてカウント
    #[serde(default = "default_debounce_frames")]
    pub debounce_frames: usize,
}

fn default_input_gain() -> f32 {
    1.0
}

fn default_smoothing_alpha() -> f32 {
    0.1
}

fn default_relative_threshold_multiplier() -> f32 {
    3.0
}

fn default_calibration_duration() -> f32 {
    0.5
}

fn default_debounce_frames() -> usize {
    3
}

/// 音声認識（STT）の設定
#[derive(Debug, Deserialize)]
pub struct SttConfig {
    /// Whisperモデルファイルのパス
    pub model_path: String,
    /// 認識言語（例: "ja", "en"）
    pub language: String,
}

/// LLM（大規模言語モデル）の設定
#[derive(Debug, Deserialize)]
pub struct LlmConfig {
    /// OllamaエンドポイントURL
    pub endpoint: String,
    /// 使用するモデル名
    pub model: String,
    /// システムプロンプト
    pub system_prompt: String,
}

/// 音声合成（TTS）の設定
#[derive(Debug, Deserialize)]
pub struct TtsConfig {
    /// VOICEVOXエンドポイントURL
    pub endpoint: String,
    /// 話者ID
    pub speaker_id: i32,
    /// 話速（0.5〜2.0）
    pub speed: f32,
}

impl Config {
    /// 設定ファイルを読み込む
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .with_context(|| format!("設定ファイルの読み込みに失敗: {}", path.display()))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("設定ファイルのパースに失敗: {}", path.display()))?;

        Ok(config)
    }
}
