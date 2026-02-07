use anyhow::Result;
use log::{debug, info};
use thiserror::Error;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::config::SttConfig;

/// 音量正規化のターゲットピーク値（0.8〜0.95推奨）
const NORMALIZATION_TARGET: f32 = 0.9;
/// 正規化をスキップする最小ピーク値（これ以下は無音とみなす）
const MIN_PEAK_FOR_NORMALIZATION: f32 = 0.001;

// === VAD (Voice Activity Detection) 設定 ===
/// VADフレームサイズ（20ms @ 16kHz = 320サンプル）
const VAD_FRAME_SIZE: usize = 320;
/// 音声検出のRMSしきい値（これ以上で音声とみなす）
const VAD_SPEECH_THRESHOLD: f32 = 0.01;
/// 音声区間の前後に追加するマージン（フレーム数）
const VAD_MARGIN_FRAMES: usize = 5;
/// 音声区間間のギャップをマージする最大フレーム数
const VAD_MAX_GAP_FRAMES: usize = 10;

/// STT処理に関するエラー
#[derive(Debug, Error)]
pub enum SttError {
    #[error("Whisperモデルの読み込みに失敗: {0}")]
    ModelLoadError(String),

    #[error("音声認識処理に失敗: {0}")]
    TranscriptionError(String),
}

/// Whisperを使用した音声認識エンジン
pub struct WhisperStt {
    ctx: WhisperContext,
    language: String,
}

impl WhisperStt {
    /// 設定からWhisperSttインスタンスを生成
    ///
    /// # Arguments
    /// * `config` - STT設定
    ///
    /// # Returns
    /// 初期化されたWhisperSttインスタンス
    pub fn new(config: &SttConfig) -> Result<Self> {
        info!("Whisperモデルを読み込み中: {}", config.model_path);

        let ctx = WhisperContext::new_with_params(&config.model_path, WhisperContextParameters::default())
            .map_err(|e| SttError::ModelLoadError(format!("{}: {}", config.model_path, e)))?;

        info!("Whisperモデルの読み込み完了");

        Ok(Self {
            ctx,
            language: config.language.clone(),
        })
    }

    /// 音声データをテキストに変換
    ///
    /// # Arguments
    /// * `audio` - 音声データ（f32, 16kHz, モノラル, -1.0〜1.0の範囲）
    ///
    /// # Returns
    /// 認識されたテキスト
    pub fn transcribe(&self, audio: &[f32]) -> Result<String> {
        debug!("音声認識開始: {} サンプル ({:.2}秒)", audio.len(), audio.len() as f32 / 16000.0);

        // 前処理1: VAD（無音区間除去）
        let vad_audio = Self::apply_vad(audio);
        if vad_audio.is_empty() {
            debug!("VAD: 音声区間が検出されませんでした");
            return Ok(String::new());
        }

        // 前処理2: 音量正規化（精度改善の最重要項目）
        let normalized_audio = Self::normalize_audio(&vad_audio);

        // BeamSearch使用（精度向上、速度はやや低下）
        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: 1.0,
        });
        params.set_language(Some(&self.language));
        // temperature = 0 でランダム性を排除し安定化
        params.set_temperature(0.0);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let mut state = self.ctx.create_state()
            .map_err(|e| SttError::TranscriptionError(format!("状態の作成に失敗: {}", e)))?;

        state.full(params, &normalized_audio)
            .map_err(|e| SttError::TranscriptionError(format!("認識処理に失敗: {}", e)))?;

        let num_segments = state.full_n_segments();

        let mut result = String::new();
        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                // 無音確率が高いセグメントはスキップ（ハルシネーション防止）
                let no_speech_prob = segment.no_speech_probability();
                if no_speech_prob > 0.6 {
                    debug!("セグメント{}: 無音確率が高いためスキップ (no_speech_prob={:.2})", i, no_speech_prob);
                    continue;
                }

                if let Ok(text) = segment.to_str_lossy() {
                    debug!("セグメント{}: \"{}\" (no_speech_prob={:.2})", i, text, no_speech_prob);
                    result.push_str(&text);
                }
            }
        }

        let result = result.trim().to_string();
        debug!("音声認識完了: \"{}\"", result);

        Ok(result)
    }

    /// 音声データの音量正規化
    ///
    /// ピーク振幅を0.9に正規化することで認識精度を向上させる。
    /// Whisperは入力振幅不足に非常に敏感なため、これは最重要の前処理。
    fn normalize_audio(audio: &[f32]) -> Vec<f32> {
        if audio.is_empty() {
            return Vec::new();
        }

        // ピーク振幅を検出
        let max_amplitude = audio.iter().fold(0.0_f32, |a, &b| a.max(b.abs()));

        // 無音に近い場合は正規化をスキップ
        if max_amplitude < MIN_PEAK_FOR_NORMALIZATION {
            debug!("音量が低すぎるため正規化をスキップ (peak={:.6})", max_amplitude);
            return audio.to_vec();
        }

        // ゲイン計算
        let gain = NORMALIZATION_TARGET / max_amplitude;

        // ゲインが1.0未満（既に十分な音量）の場合もスキップ
        if gain <= 1.0 {
            debug!("音量が十分なため正規化不要 (peak={:.3}, gain={:.2})", max_amplitude, gain);
            return audio.to_vec();
        }

        debug!(
            "音量正規化: peak={:.3} -> {:.3} (gain={:.2}x)",
            max_amplitude,
            NORMALIZATION_TARGET,
            gain
        );

        // 正規化実行（クリッピング防止付き）
        audio
            .iter()
            .map(|&s| (s * gain).clamp(-1.0, 1.0))
            .collect()
    }

    /// VAD（Voice Activity Detection）による無音区間除去
    ///
    /// 音声区間のみを抽出することで、Whisperの誤認識を防ぐ。
    /// エネルギーベースのシンプルなVADを使用。
    fn apply_vad(audio: &[f32]) -> Vec<f32> {
        if audio.is_empty() {
            return Vec::new();
        }

        let num_frames = audio.len() / VAD_FRAME_SIZE;
        if num_frames == 0 {
            // フレーム数が足りない場合はそのまま返す
            return audio.to_vec();
        }

        // 各フレームのRMSを計算し、音声フレームを検出
        let mut is_speech: Vec<bool> = Vec::with_capacity(num_frames);
        for i in 0..num_frames {
            let start = i * VAD_FRAME_SIZE;
            let end = start + VAD_FRAME_SIZE;
            let frame = &audio[start..end];

            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / VAD_FRAME_SIZE as f32).sqrt();
            is_speech.push(rms >= VAD_SPEECH_THRESHOLD);
        }

        // 音声区間の前後にマージンを追加
        let mut expanded_speech = is_speech.clone();
        for i in 0..num_frames {
            if is_speech[i] {
                // 前方マージン
                let margin_start = i.saturating_sub(VAD_MARGIN_FRAMES);
                for j in margin_start..i {
                    expanded_speech[j] = true;
                }
                // 後方マージン
                let margin_end = (i + VAD_MARGIN_FRAMES + 1).min(num_frames);
                for j in (i + 1)..margin_end {
                    expanded_speech[j] = true;
                }
            }
        }

        // 小さなギャップをマージ
        let mut merged_speech = expanded_speech.clone();
        let mut gap_start: Option<usize> = None;
        for i in 0..num_frames {
            if expanded_speech[i] {
                if let Some(start) = gap_start {
                    let gap_len = i - start;
                    if gap_len <= VAD_MAX_GAP_FRAMES {
                        // ギャップが短い場合は埋める
                        for j in start..i {
                            merged_speech[j] = true;
                        }
                    }
                }
                gap_start = None;
            } else if gap_start.is_none() && i > 0 && expanded_speech[i - 1] {
                gap_start = Some(i);
            }
        }

        // 音声区間を抽出
        let mut result = Vec::new();
        let mut in_speech = false;
        let mut speech_start = 0;

        for i in 0..num_frames {
            if merged_speech[i] && !in_speech {
                in_speech = true;
                speech_start = i * VAD_FRAME_SIZE;
            } else if !merged_speech[i] && in_speech {
                in_speech = false;
                let speech_end = i * VAD_FRAME_SIZE;
                result.extend_from_slice(&audio[speech_start..speech_end]);
            }
        }

        // 最後のフレームまで音声が続いている場合
        if in_speech {
            let speech_end = num_frames * VAD_FRAME_SIZE;
            result.extend_from_slice(&audio[speech_start..speech_end]);
        }

        // 残りのサンプル（フレームに収まらなかった部分）
        let remaining_start = num_frames * VAD_FRAME_SIZE;
        if remaining_start < audio.len() && in_speech {
            result.extend_from_slice(&audio[remaining_start..]);
        }

        let original_duration = audio.len() as f32 / 16000.0;
        let result_duration = result.len() as f32 / 16000.0;
        let speech_frames = merged_speech.iter().filter(|&&x| x).count();

        debug!(
            "VAD: {:.2}秒 -> {:.2}秒 (音声フレーム: {}/{})",
            original_duration,
            result_duration,
            speech_frames,
            num_frames
        );

        result
    }
}
