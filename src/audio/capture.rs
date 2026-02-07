use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, Stream, StreamConfig};
use log::{debug, info, warn};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// リングバッファの容量（2秒分 @ 48kHz = 96000サンプル）
/// デバイスレートが48kHzの場合でも十分な容量を確保
const RING_BUFFER_CAPACITY: usize = 96000;

/// ウェイクワード検出後のlookbackサンプル数（0.5秒分 @ 48kHz）
const LOOKBACK_SAMPLES: usize = 24000;

/// 音声キャプチャに関するエラー
#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("入力デバイスが見つかりません")]
    NoInputDevice,

    #[error("デバイス設定の取得に失敗: {0}")]
    ConfigError(String),

    #[error("ストリームの作成に失敗: {0}")]
    StreamError(String),

    #[error("録音中にエラーが発生: {0}")]
    RecordingError(String),
}

/// リングバッファの内部状態
struct AudioCaptureInner {
    ring_buffer: Vec<f32>,
    write_pos: usize,
    total_written: u64,
    /// ストリーミング用の読み取り位置（total_written単位）
    stream_read_pos: u64,
}

impl AudioCaptureInner {
    fn new() -> Self {
        Self {
            ring_buffer: vec![0.0; RING_BUFFER_CAPACITY],
            write_pos: 0,
            total_written: 0,
            stream_read_pos: 0,
        }
    }

    /// リングバッファにサンプルを書き込む
    fn write_samples(&mut self, samples: &[f32]) {
        for &sample in samples {
            self.ring_buffer[self.write_pos] = sample;
            self.write_pos = (self.write_pos + 1) % RING_BUFFER_CAPACITY;
            self.total_written += 1;
        }
    }

    /// リングバッファから最新のN個のサンプルを読み取る（従来方式）
    fn read_latest(&self, num_samples: usize) -> Vec<f32> {
        let num_samples = num_samples.min(RING_BUFFER_CAPACITY);
        let available = (self.total_written as usize).min(RING_BUFFER_CAPACITY);
        let actual_samples = num_samples.min(available);

        if actual_samples == 0 {
            return Vec::new();
        }

        let mut result = Vec::with_capacity(actual_samples);
        let start_pos = (self.write_pos + RING_BUFFER_CAPACITY - actual_samples) % RING_BUFFER_CAPACITY;

        for i in 0..actual_samples {
            result.push(self.ring_buffer[(start_pos + i) % RING_BUFFER_CAPACITY]);
        }

        result
    }

    /// ストリーミング読み取り用: まだ読み取っていないサンプル数を返す
    fn unread_samples(&self) -> usize {
        if self.total_written <= self.stream_read_pos {
            return 0;
        }
        let unread = self.total_written - self.stream_read_pos;
        // リングバッファ容量を超えていたらデータロスト
        unread.min(RING_BUFFER_CAPACITY as u64) as usize
    }

    /// ストリーミング読み取り: 連続した次のN個のサンプルを返す（重複なし）
    fn read_stream(&mut self, num_samples: usize) -> Vec<f32> {
        let available = self.unread_samples();
        let to_read = num_samples.min(available);

        if to_read == 0 {
            return Vec::new();
        }

        // 読み取り位置がオーバーライトされた場合、最古の有効位置にジャンプ
        if self.total_written > RING_BUFFER_CAPACITY as u64 {
            let oldest_valid = self.total_written - RING_BUFFER_CAPACITY as u64;
            if self.stream_read_pos < oldest_valid {
                self.stream_read_pos = oldest_valid;
            }
        }

        // シンプルな計算: 論理位置をバッファインデックスに変換
        let buffer_start = (self.stream_read_pos % RING_BUFFER_CAPACITY as u64) as usize;

        let mut result = Vec::with_capacity(to_read);
        for i in 0..to_read {
            result.push(self.ring_buffer[(buffer_start + i) % RING_BUFFER_CAPACITY]);
        }

        // 読み取り位置を進める
        self.stream_read_pos += to_read as u64;

        result
    }

    /// ストリーミング読み取り位置をリセット（現在位置に同期）
    fn reset_stream_position(&mut self) {
        self.stream_read_pos = self.total_written;
    }
}

/// 録音状態の管理
struct RecordingState {
    samples: Vec<f32>,
    is_recording: bool,
    speech_detected: bool,
    consecutive_silence: usize,
    silence_samples_threshold: usize,
    max_samples: usize,
    silence_threshold: f32,
    current_level: f32,
    // 無音検出改善用フィールド
    /// 平滑化されたRMS
    smoothed_rms: f32,
    /// 平滑化係数（0.1が推奨）
    smoothing_alpha: f32,
    /// ノイズフロア（キャリブレーション後に設定）
    noise_floor: f32,
    /// キャリブレーション完了フラグ
    calibration_complete: bool,
    /// キャリブレーション期間（サンプル数）
    calibration_duration: usize,
    /// キャリブレーション中のRMS合計
    calibration_rms_sum: f32,
    /// キャリブレーション中のRMSカウント
    calibration_rms_count: usize,
    /// 相対閾値の乗数
    relative_threshold_multiplier: f32,
    /// 連続無音フレーム数（デバウンス用）
    silent_frame_count: usize,
    /// デバウンス閾値
    debounce_frames: usize,
    /// サンプルレート（デバッグログ用）
    sample_rate: u32,
}

impl RecordingState {
    fn new() -> Self {
        Self {
            samples: Vec::new(),
            is_recording: false,
            speech_detected: false,
            consecutive_silence: 0,
            silence_samples_threshold: 0,
            max_samples: 0,
            silence_threshold: 0.01,
            current_level: 0.0,
            // 無音検出改善用フィールドの初期化
            smoothed_rms: 0.0,
            smoothing_alpha: 0.1,
            noise_floor: 0.0,
            calibration_complete: false,
            calibration_duration: 0,
            calibration_rms_sum: 0.0,
            calibration_rms_count: 0,
            relative_threshold_multiplier: 3.0,
            silent_frame_count: 0,
            debounce_frames: 3,
            sample_rate: 16000,
        }
    }

    fn start(
        &mut self,
        lookback_samples: Vec<f32>,
        max_samples: usize,
        silence_samples_threshold: usize,
        silence_threshold: f32,
        sample_rate: u32,
        smoothing_alpha: f32,
        relative_threshold_multiplier: f32,
        calibration_duration: f32,
        debounce_frames: usize,
    ) {
        self.samples = lookback_samples;
        self.is_recording = true;
        self.speech_detected = false;
        self.consecutive_silence = 0;
        self.silence_samples_threshold = silence_samples_threshold;
        self.max_samples = max_samples;
        self.silence_threshold = silence_threshold;
        self.current_level = 0.0;
        // 無音検出改善用フィールドのリセット
        self.smoothed_rms = 0.0;
        self.smoothing_alpha = smoothing_alpha;
        self.noise_floor = 0.0;
        self.calibration_complete = false;
        self.calibration_duration = (calibration_duration * sample_rate as f32) as usize;
        self.calibration_rms_sum = 0.0;
        self.calibration_rms_count = 0;
        self.relative_threshold_multiplier = relative_threshold_multiplier;
        self.silent_frame_count = 0;
        self.debounce_frames = debounce_frames;
        self.sample_rate = sample_rate;
    }

    fn stop(&mut self) -> Vec<f32> {
        self.is_recording = false;
        std::mem::take(&mut self.samples)
    }

    fn add_samples(&mut self, samples: &[f32]) {
        if !self.is_recording {
            return;
        }

        self.samples.extend_from_slice(samples);

        // RMS計算
        if !samples.is_empty() {
            let frame_rms =
                (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();

            // 指数移動平均によるRMS平滑化
            if self.smoothed_rms == 0.0 {
                self.smoothed_rms = frame_rms;
            } else {
                self.smoothed_rms =
                    self.smoothing_alpha * frame_rms + (1.0 - self.smoothing_alpha) * self.smoothed_rms;
            }

            self.current_level = self.smoothed_rms;

            // キャリブレーション期間中
            if !self.calibration_complete {
                self.calibration_rms_sum += frame_rms;
                self.calibration_rms_count += 1;

                // キャリブレーション完了判定（サンプル数ベース）
                if self.samples.len() >= self.calibration_duration {
                    if self.calibration_rms_count > 0 {
                        self.noise_floor =
                            self.calibration_rms_sum / self.calibration_rms_count as f32;
                        // ノイズフロアの最小値を設定（極端に静かな環境対策）
                        self.noise_floor = self.noise_floor.max(0.001);
                    } else {
                        self.noise_floor = self.silence_threshold;
                    }
                    self.calibration_complete = true;

                    let effective_threshold =
                        self.noise_floor * self.relative_threshold_multiplier;
                    debug!(
                        "Noise floor calibration complete: {:.4}, effective threshold: {:.4}",
                        self.noise_floor, effective_threshold
                    );
                }
                return; // キャリブレーション中は無音判定しない
            }

            // キャリブレーション後：相対閾値による判定
            let effective_threshold = self.noise_floor * self.relative_threshold_multiplier;

            if self.smoothed_rms >= effective_threshold {
                // 発話検出
                self.speech_detected = true;
                self.consecutive_silence = 0;
                self.silent_frame_count = 0;
            } else if self.speech_detected {
                // 無音フレームのデバウンス処理
                self.silent_frame_count += 1;

                // 連続した無音フレームがデバウンス閾値を超えたら無音としてカウント
                if self.silent_frame_count >= self.debounce_frames {
                    self.consecutive_silence += samples.len();
                }
            }
        }
    }

    fn should_stop(&self) -> bool {
        if !self.is_recording {
            return true;
        }
        if self.samples.len() >= self.max_samples {
            return true;
        }
        if self.speech_detected && self.consecutive_silence >= self.silence_samples_threshold {
            return true;
        }
        false
    }
}

/// マイクからの音声キャプチャを管理（永続ストリーム版）
pub struct AudioCapture {
    #[allow(dead_code)]
    device: Device,
    #[allow(dead_code)]
    config: StreamConfig,
    sample_rate: u32,
    target_sample_rate: u32,
    _stream: Stream,
    inner: Arc<Mutex<AudioCaptureInner>>,
    recording_state: Arc<Mutex<RecordingState>>,
    recording_active: Arc<AtomicBool>,
    resample_ratio: f64,
    input_gain: f32,
    // 無音検出改善用設定
    smoothing_alpha: f32,
    relative_threshold_multiplier: f32,
    calibration_duration: f32,
    debounce_frames: usize,
}

impl AudioCapture {
    /// デフォルトの入力デバイスでAudioCaptureを初期化
    /// ストリームは即座に開始され、永続的に動作する
    pub fn new(
        target_sample_rate: u32,
        input_gain: f32,
        smoothing_alpha: f32,
        relative_threshold_multiplier: f32,
        calibration_duration: f32,
        debounce_frames: usize,
    ) -> Result<Self> {
        let host = cpal::default_host();

        let device = host
            .default_input_device()
            .ok_or(CaptureError::NoInputDevice)?;

        let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
        info!("入力デバイス: {}", device_name);

        let supported_configs = device
            .supported_input_configs()
            .map_err(|e| CaptureError::ConfigError(e.to_string()))?;

        let mut best_config = None;
        for config in supported_configs {
            if config.channels() == 1
                && config.min_sample_rate().0 <= target_sample_rate
                && config.max_sample_rate().0 >= target_sample_rate
            {
                best_config = Some(config.with_sample_rate(SampleRate(target_sample_rate)));
                break;
            }
        }

        let supported_config = best_config.unwrap_or_else(|| {
            let default_config = device
                .default_input_config()
                .expect("デフォルト入力設定の取得に失敗");
            warn!(
                "目標サンプルレート{}Hzに対応した設定が見つかりません。デフォルト設定を使用: {}Hz, {}ch（リサンプリングします）",
                target_sample_rate,
                default_config.sample_rate().0,
                default_config.channels()
            );
            default_config
        });

        let sample_rate = supported_config.sample_rate().0;
        let config: StreamConfig = supported_config.into();
        let channels = config.channels as usize;

        info!(
            "音声キャプチャ設定: {}Hz, {}ch, gain={:.1}x (永続ストリーム)",
            sample_rate, config.channels, input_gain
        );

        let resample_ratio = sample_rate as f64 / target_sample_rate as f64;

        // 共有状態の初期化
        let inner = Arc::new(Mutex::new(AudioCaptureInner::new()));
        let recording_state = Arc::new(Mutex::new(RecordingState::new()));
        let recording_active = Arc::new(AtomicBool::new(false));

        // コールバック用のクローン
        let inner_clone = Arc::clone(&inner);
        let recording_state_clone = Arc::clone(&recording_state);
        let recording_active_clone = Arc::clone(&recording_active);
        let gain = input_gain;

        let err_flag = Arc::new(Mutex::new(None::<String>));
        let err_flag_clone = Arc::clone(&err_flag);

        // 永続ストリームの作成
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // マルチチャンネルをモノラルに変換し、ゲインを適用
                    let mono_samples: Vec<f32> = data
                        .chunks(channels)
                        .map(|chunk| {
                            let sample = chunk.iter().sum::<f32>() / channels as f32;
                            // ゲイン適用 & クリッピング防止
                            (sample * gain).clamp(-1.0, 1.0)
                        })
                        .collect();

                    // リングバッファに書き込み
                    {
                        let mut inner = inner_clone.lock().unwrap();
                        inner.write_samples(&mono_samples);
                    }

                    // 録音中の場合は録音バッファにも追加
                    if recording_active_clone.load(Ordering::Relaxed) {
                        let mut state = recording_state_clone.lock().unwrap();
                        state.add_samples(&mono_samples);
                    }
                },
                move |err| {
                    let mut error = err_flag_clone.lock().unwrap();
                    *error = Some(err.to_string());
                },
                None,
            )
            .map_err(|e| CaptureError::StreamError(e.to_string()))?;

        // ストリームを開始
        stream
            .play()
            .map_err(|e| CaptureError::StreamError(e.to_string()))?;

        info!("永続オーディオストリームを開始しました");

        let capture = Self {
            device,
            config,
            sample_rate,
            target_sample_rate,
            _stream: stream,
            inner,
            recording_state,
            recording_active,
            resample_ratio,
            input_gain,
            smoothing_alpha,
            relative_threshold_multiplier,
            calibration_duration,
            debounce_frames,
        };

        // 初期化時にバッファが十分に蓄積されるまで待機
        // マイクのウォームアップ期間を考慮して2秒待機
        let warmup_samples = (sample_rate as f64 * 2.0) as u64; // 2秒分
        let start = std::time::Instant::now();
        loop {
            let written = {
                let inner_guard = capture.inner.lock().unwrap();
                inner_guard.total_written
            };
            if written >= warmup_samples {
                info!(
                    "マイクウォームアップ完了: {} サンプル蓄積",
                    written
                );
                break;
            }
            if start.elapsed().as_secs() > 5 {
                warn!("マイクウォームアップタイムアウト: {} サンプルのみ蓄積", written);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // ウォームアップ後、バッファをクリアして新しいデータから開始
        // これにより起動時のノイズやポップ音による誤検出を防ぐ
        {
            let mut inner_guard = capture.inner.lock().unwrap();
            inner_guard.ring_buffer.fill(0.0);
            inner_guard.write_pos = 0;
            inner_guard.total_written = 0;
            inner_guard.stream_read_pos = 0;
        }
        info!("オーディオバッファをクリアしました（クリーンスタート）");

        // クリア後、検出に必要な最小限のデータを再蓄積
        let min_samples_needed = (sample_rate as f64 * 0.5) as u64; // 0.5秒分
        let start = std::time::Instant::now();
        loop {
            let written = {
                let inner_guard = capture.inner.lock().unwrap();
                inner_guard.total_written
            };
            if written >= min_samples_needed {
                info!(
                    "オーディオバッファ準備完了: {} サンプル蓄積",
                    written
                );
                break;
            }
            if start.elapsed().as_secs() > 3 {
                warn!("オーディオバッファ準備タイムアウト");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        Ok(capture)
    }

    /// リングバッファから最新のN個のサンプルを取得（ウェイクワード検出用）
    /// num_samples: target_sample_rate (16kHz) でのサンプル数
    pub fn get_samples(&self, num_samples: usize) -> Vec<i16> {
        // デバイスレートでのサンプル数計算
        let device_samples = (num_samples as f64 * self.resample_ratio).ceil() as usize;

        // リングバッファから読み取り（十分なデータがあるか確認）
        let samples = {
            let inner = self.inner.lock().unwrap();
            let available = (inner.total_written as usize).min(RING_BUFFER_CAPACITY);
            if available < device_samples {
                // 十分なデータがない場合は利用可能な分だけ返す
                debug!(
                    "get_samples: 十分なデータなし available={} required={}",
                    available, device_samples
                );
            }
            inner.read_latest(device_samples)
        };

        if samples.is_empty() {
            // バッファが完全に空の場合（通常は起動直後のみ）
            debug!("get_samples: バッファが空です");
            return vec![0i16; num_samples];
        }

        // リサンプル & i16変換
        let resampled = if self.sample_rate != self.target_sample_rate {
            resample(&samples, self.sample_rate, self.target_sample_rate)
        } else {
            samples
        };

        // f32 [-1.0, 1.0] を i16 に変換
        let mut i16_samples: Vec<i16> = resampled
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect();

        // サンプル数を正確に num_samples に合わせる
        i16_samples.resize(num_samples, 0);

        i16_samples
    }

    /// 指定されたサンプル数を録音してi16形式で返す（ストリーミング用）
    /// ウェイクワード検出に使用 - 連続したフレームを返す（重複なし）
    pub fn record_samples(&self, num_samples: usize) -> Result<Vec<i16>> {
        // デバイスレートでのサンプル数計算
        let device_samples = (num_samples as f64 * self.resample_ratio).ceil() as usize;
        let start = std::time::Instant::now();

        // 必要なサンプル数が蓄積されるまで待機
        loop {
            let unread = {
                let inner = self.inner.lock().unwrap();
                inner.unread_samples()
            };

            if unread >= device_samples {
                break;
            }

            if start.elapsed().as_secs() > 2 {
                debug!(
                    "record_samples timeout: unread={} required={}",
                    unread, device_samples
                );
                // タイムアウト時は利用可能な分だけで進む
                break;
            }

            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        // ストリーミング読み取り（連続、重複なし）
        let samples = {
            let mut inner = self.inner.lock().unwrap();
            inner.read_stream(device_samples)
        };

        if samples.is_empty() {
            debug!("record_samples: no samples available");
            return Ok(vec![0i16; num_samples]);
        }

        // リサンプル
        let resampled = if self.sample_rate != self.target_sample_rate {
            resample(&samples, self.sample_rate, self.target_sample_rate)
        } else {
            samples
        };

        // f32 [-1.0, 1.0] を i16 に変換
        let mut i16_samples: Vec<i16> = resampled
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect();

        // サンプル数を正確に num_samples に合わせる
        i16_samples.resize(num_samples, 0);

        Ok(i16_samples)
    }

    /// ストリーミング読み取り位置をリセット（現在位置に同期）
    pub fn reset_stream_position(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.reset_stream_position();
    }

    /// 録音を開始（lookback込み）
    fn start_recording(
        &self,
        max_duration_secs: f32,
        silence_threshold: f32,
        silence_duration_secs: f32,
    ) {
        let max_samples = (max_duration_secs * self.sample_rate as f32) as usize;
        let silence_samples = (silence_duration_secs * self.sample_rate as f32) as usize;

        // lookbackサンプルをリングバッファから取得
        let lookback = {
            let inner = self.inner.lock().unwrap();
            let lookback_device_samples = LOOKBACK_SAMPLES.min(inner.total_written as usize);
            inner.read_latest(lookback_device_samples)
        };

        // 録音状態を初期化
        {
            let mut state = self.recording_state.lock().unwrap();
            state.start(
                lookback,
                max_samples,
                silence_samples,
                silence_threshold,
                self.sample_rate,
                self.smoothing_alpha,
                self.relative_threshold_multiplier,
                self.calibration_duration,
                self.debounce_frames,
            );
        }

        // 録音フラグを立てる
        self.recording_active.store(true, Ordering::Relaxed);
    }

    /// 録音が完了したかどうかをチェック
    fn is_recording_complete(&self) -> bool {
        let state = self.recording_state.lock().unwrap();
        state.should_stop()
    }

    /// 現在の音声レベルを取得
    fn get_current_level(&self) -> (f32, bool) {
        let state = self.recording_state.lock().unwrap();
        (state.current_level, state.speech_detected)
    }

    /// 録音を停止し、結果を返す
    fn stop_recording(&self) -> Vec<f32> {
        self.recording_active.store(false, Ordering::Relaxed);

        let recorded = {
            let mut state = self.recording_state.lock().unwrap();
            state.stop()
        };

        // リサンプリング
        if self.sample_rate != self.target_sample_rate {
            resample(&recorded, self.sample_rate, self.target_sample_rate)
        } else {
            recorded
        }
    }

    /// 無音検出で自動停止する録音を実行（静かなモード - ウェイクワード検出用）
    pub fn record_until_silence(
        &self,
        max_duration_secs: f32,
        silence_threshold: f32,
        silence_duration_secs: f32,
    ) -> Result<Vec<f32>> {
        self.record_internal(max_duration_secs, silence_threshold, silence_duration_secs, true)
    }

    /// 無音検出で自動停止する録音を実行（詳細表示モード - コマンド入力用）
    pub fn record_with_feedback(
        &self,
        max_duration_secs: f32,
        silence_threshold: f32,
        silence_duration_secs: f32,
    ) -> Result<Vec<f32>> {
        self.record_internal(max_duration_secs, silence_threshold, silence_duration_secs, false)
    }

    fn record_internal(
        &self,
        max_duration_secs: f32,
        silence_threshold: f32,
        silence_duration_secs: f32,
        quiet: bool,
    ) -> Result<Vec<f32>> {
        // 録音開始
        self.start_recording(max_duration_secs, silence_threshold, silence_duration_secs);

        if !quiet {
            println!();
            println!(
                ">>> Recording... (max {}s, silence {}s to stop)",
                max_duration_secs, silence_duration_secs
            );
            println!(">>> Speak now!");
            println!();
        }

        let mut last_print = std::time::Instant::now();

        // 録音完了を待機
        while !self.is_recording_complete() {
            std::thread::sleep(std::time::Duration::from_millis(50));

            if !quiet && last_print.elapsed().as_millis() >= 100 {
                let (level, speech_detected) = self.get_current_level();
                let bars = (level * 50.0).min(50.0) as usize;
                let meter: String = "#".repeat(bars) + &"-".repeat(50 - bars);
                let speech = if speech_detected {
                    "[SPEECH]"
                } else {
                    "[      ]"
                };
                print!("\r  {} |{}| {:.3}", speech, meter, level);
                let _ = io::stdout().flush();
                last_print = std::time::Instant::now();
            }
        }

        if !quiet {
            println!();
            println!();
        }

        // 録音停止と結果取得
        let recorded_samples = self.stop_recording();

        if !quiet {
            let duration = recorded_samples.len() as f32 / self.target_sample_rate as f32;
            info!(
                "録音完了: {:.2}秒 ({} サンプル)",
                duration,
                recorded_samples.len()
            );
        }

        Ok(recorded_samples)
    }
}

fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let new_len = (samples.len() as f64 / ratio) as usize;
    let mut resampled = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_idx = i as f64 * ratio;
        let idx_floor = src_idx.floor() as usize;
        let idx_ceil = (idx_floor + 1).min(samples.len() - 1);
        let frac = (src_idx - idx_floor as f64) as f32;

        let sample = samples[idx_floor] * (1.0 - frac) + samples[idx_ceil] * frac;
        resampled.push(sample);
    }

    resampled
}
