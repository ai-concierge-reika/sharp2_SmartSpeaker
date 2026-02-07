use anyhow::Result;
use log::{debug, info};
use rustpotter::{Rustpotter, RustpotterConfig, SampleFormat};
use std::io::{self, Write};

use crate::audio::AudioCapture;
use crate::config::WakewordConfig;

/// ウェイクワード検出結果
pub struct WakewordResult {
    /// 検出されたウェイクワード名
    pub keyword: String,
    /// 検出スコア（0.0〜1.0）
    pub score: f32,
}

/// 起動直後にスキップするフレーム数（誤検出防止）
/// 100 (~0.3秒) → 300 (~1秒) に増加
const WARMUP_FRAMES: u64 = 300;

// === 音量正規化設定 ===
/// 正規化後のターゲットピーク（i16範囲の約85%）
const NORMALIZE_TARGET_PEAK: i16 = 28000;
/// 正規化をスキップする最小ピーク値（これ以下は無音とみなす）
const NORMALIZE_MIN_PEAK: i16 = 100;

// === VAD設定 ===
/// VADのRMSしきい値（i16スケール、これ以上で音声とみなす）
const VAD_THRESHOLD_I16: f32 = 300.0;
/// VADで無音と判定された場合のゲイン係数（完全に0にはしない）
const VAD_SILENCE_GAIN: f32 = 0.1;

/// Rustpotterベースのウェイクワード検出器
pub struct WakewordDetector {
    rustpotter: Rustpotter,
    samples_per_frame: usize,
}

impl WakewordDetector {
    /// 設定からWakewordDetectorを生成
    pub fn new(config: &WakewordConfig) -> Result<Self> {
        // モデルファイルの存在確認
        let wakeword_path = std::path::Path::new(&config.wakeword_path);
        if !wakeword_path.exists() {
            // カレントディレクトリからの相対パスを試す
            let cwd = std::env::current_dir().unwrap_or_default();
            let full_path = cwd.join(&config.wakeword_path);
            if !full_path.exists() {
                return Err(anyhow::anyhow!(
                    "ウェイクワードファイルが見つかりません: {} (cwd: {})",
                    config.wakeword_path,
                    cwd.display()
                ));
            }
            info!("ウェイクワードファイル解決: {} -> {}", config.wakeword_path, full_path.display());
        }

        // Rustpotter設定を初期化
        let mut rustpotter_config = RustpotterConfig::default();

        // 音声フォーマット設定（16kHz, mono, i16）
        rustpotter_config.fmt.sample_format = SampleFormat::I16;
        rustpotter_config.fmt.sample_rate = 16000;
        rustpotter_config.fmt.channels = 1;

        // 検出感度を設定
        rustpotter_config.detector.threshold = config.threshold;
        rustpotter_config.detector.avg_threshold = config.avg_threshold;
        // 連続検出回数を設定（単発の誤検出を防ぐ）
        rustpotter_config.detector.min_scores = config.min_scores;

        info!(
            "Rustpotter設定: threshold={}, avg_threshold={}, min_scores={}",
            config.threshold, config.avg_threshold, config.min_scores
        );

        // Rustpotterインスタンスを作成
        let mut rustpotter = Rustpotter::new(&rustpotter_config)
            .map_err(|e| anyhow::anyhow!("Rustpotterの初期化に失敗: {}", e))?;

        // ウェイクワードファイルを読み込み（keyはファイル名から自動生成）
        let wakeword_key = std::path::Path::new(&config.wakeword_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("wakeword");
        rustpotter
            .add_wakeword_from_file(wakeword_key, &config.wakeword_path)
            .map_err(|e| anyhow::anyhow!("ウェイクワードファイルの読み込みに失敗: {} - {}", config.wakeword_path, e))?;

        let samples_per_frame = rustpotter.get_samples_per_frame();

        info!(
            "ウェイクワード検出器初期化完了: keyword=\"{}\", samples_per_frame={}, frame_duration={:.1}ms",
            wakeword_key,
            samples_per_frame,
            samples_per_frame as f32 / 16.0 // 16kHz -> ms
        );

        Ok(Self {
            rustpotter,
            samples_per_frame,
        })
    }

    /// ウェイクワードを検出するまで待機
    pub fn wait_for_wakeword(&mut self, capture: &AudioCapture) -> Result<WakewordResult> {
        println!();
        println!("========================================");
        println!("  Waiting for wakeword...");
        println!("========================================");
        println!();

        // ストリーム読み取り位置をリセット（連続フレーム読み取りのため）
        capture.reset_stream_position();
        debug!("ストリーム読み取り位置をリセット");

        let mut frame_count = 0u64;
        let mut max_rms_seen: f32 = 0.0;
        let mut max_score_seen: f32 = 0.0;

        loop {
            frame_count += 1;

            // フレーム分の音声を取得（連続、重複なし）
            let raw_samples = capture.record_samples(self.samples_per_frame)?;

            // 前処理パイプライン（正規化 + VAD）
            let samples = Self::preprocess_samples(&raw_samples);

            // デバッグ: 音声レベルとサンプル数（前処理後）
            let rms: f32 = if !samples.is_empty() {
                let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
                ((sum / samples.len() as f64).sqrt() / i16::MAX as f64) as f32
            } else {
                0.0
            };

            // ウォームアップ期間中は検出をスキップ
            if frame_count <= WARMUP_FRAMES {
                if frame_count == 1 {
                    print!("\r  [Warming up] frames:{}/{}    ", frame_count, WARMUP_FRAMES);
                } else if frame_count % 10 == 0 {
                    print!("\r  [Warming up] frames:{}/{}    ", frame_count, WARMUP_FRAMES);
                }
                let _ = io::stdout().flush();

                // Rustpotterの内部状態を更新するが、検出結果は無視
                let _ = self.rustpotter.process_samples(samples);
                continue;
            }

            // ウォームアップ完了後、最初のフレームでメッセージ表示
            if frame_count == WARMUP_FRAMES + 1 {
                println!();
                println!("  [Ready] Say the wakeword!");
            }

            // サンプル統計情報（デバッグ用）
            let sample_max = samples.iter().max().copied().unwrap_or(0);
            let sample_min = samples.iter().min().copied().unwrap_or(0);

            // Rustpotterで検出処理
            let detection = self.rustpotter.process_samples(samples.clone());

            // 部分検出スコアを取得（閾値未達でもスコアを確認）
            let partial = self.rustpotter.get_partial_detection();
            let partial_score = partial.as_ref().map(|p| p.score).unwrap_or(0.0);

            // 最大値を追跡（診断用）
            if rms > max_rms_seen {
                max_rms_seen = rms;
            }
            if partial_score > max_score_seen {
                max_score_seen = partial_score;
            }

            // 毎フレーム出力（部分スコアも表示、最大値も表示）
            print!(
                "\r  [Listening] rms:{:.4} (max:{:.4}) score:{:.3} (max:{:.3}) amp:[{},{}]    ",
                rms, max_rms_seen, partial_score, max_score_seen, sample_min, sample_max
            );
            let _ = io::stdout().flush();

            if let Some(detection) = detection {
                let keyword = detection.name.clone();
                let score = detection.score;

                println!();
                println!("  >>> WAKEWORD DETECTED! <<<");
                info!(
                    "ウェイクワード検出 (Rustpotter): keyword=\"{}\", score={:.3}",
                    keyword, score
                );
                println!("  Keyword: \"{}\"", keyword);
                println!("  Score: {:.3}", score);
                println!();

                return Ok(WakewordResult { keyword, score });
            }

            debug!("検出なし (処理継続)");
        }
    }

    /// フレームあたりのサンプル数を取得
    pub fn get_samples_per_frame(&self) -> usize {
        self.samples_per_frame
    }

    /// 音量正規化（i16サンプル用）
    ///
    /// ピーク振幅を目標値（28000）に正規化する。
    /// rustpotterは入力振幅不足に敏感なため、これは重要な前処理。
    fn normalize_samples(samples: &[i16]) -> Vec<i16> {
        if samples.is_empty() {
            return Vec::new();
        }

        // ピーク振幅を検出
        let max_amplitude = samples.iter().map(|&s| s.abs()).max().unwrap_or(0);

        // 無音に近い場合は正規化をスキップ
        if max_amplitude < NORMALIZE_MIN_PEAK {
            return samples.to_vec();
        }

        // ゲイン計算（目標ピーク / 現在ピーク）
        let gain = NORMALIZE_TARGET_PEAK as f32 / max_amplitude as f32;

        // ゲインが1.0以下（既に十分な音量）の場合はスキップ
        if gain <= 1.0 {
            return samples.to_vec();
        }

        debug!(
            "Wakeword正規化: peak={} -> {} (gain={:.2}x)",
            max_amplitude, NORMALIZE_TARGET_PEAK, gain
        );

        // 正規化実行（クリッピング防止付き）
        samples
            .iter()
            .map(|&s| {
                let amplified = (s as f32 * gain) as i32;
                amplified.clamp(i16::MIN as i32, i16::MAX as i32) as i16
            })
            .collect()
    }

    /// 簡易VAD（Voice Activity Detection）
    ///
    /// 無音フレームを検出し、ゲインを下げることで誤検出を削減。
    /// 完全に0にはせず、低ゲインで通すことで連続フレーム供給を維持。
    fn apply_vad(samples: &[i16]) -> Vec<i16> {
        if samples.is_empty() {
            return Vec::new();
        }

        // RMS計算（i16スケール）
        let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum / samples.len() as f64).sqrt() as f32;

        // 無音判定
        if rms < VAD_THRESHOLD_I16 {
            // 無音時は低ゲインで通す（連続フレーム供給のため完全に0にはしない）
            return samples
                .iter()
                .map(|&s| (s as f32 * VAD_SILENCE_GAIN) as i16)
                .collect();
        }

        samples.to_vec()
    }

    /// 前処理パイプライン（正規化 + VAD）
    fn preprocess_samples(samples: &[i16]) -> Vec<i16> {
        // 1. 音量正規化
        let normalized = Self::normalize_samples(samples);
        // 2. VAD（誤検出削減）
        Self::apply_vad(&normalized)
    }
}
