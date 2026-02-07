mod audio;
mod config;
mod llm;
mod stt;
mod tts;
mod wakeword;

use anyhow::Result;
use log::{error, info, warn};

use audio::{AudioCapture, AudioPlayback};
use config::Config;
use llm::OllamaLlm;
use stt::WhisperStt;
use tts::VoicevoxTts;
use wakeword::WakewordDetector;

fn main() -> Result<()> {
    // ログ初期化
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    info!("Smart Speaker 起動");

    // 設定ファイル読み込み
    let config = Config::load("config/settings.toml")?;
    info!("設定読み込み完了");

    // 各コンポーネントの初期化とヘルスチェック
    let llm = OllamaLlm::new(&config.llm)?;
    if !llm.health_check()? {
        error!("Ollamaサーバーに接続できません。Ollamaが起動していることを確認してください。");
        return Ok(());
    }
    info!("Ollama接続OK");

    let tts = VoicevoxTts::new(&config.tts)?;
    if !tts.health_check()? {
        error!("VOICEVOXサーバーに接続できません。VOICEVOXが起動していることを確認してください。");
        return Ok(());
    }
    info!("VOICEVOX接続OK");

    let stt = WhisperStt::new(&config.stt)?;
    info!("Whisper初期化OK");

    let mut wakeword_detector = WakewordDetector::new(&config.wakeword)?;
    info!("ウェイクワード検出器初期化OK (Rustpotter)");

    let capture = AudioCapture::new(
        config.audio.sample_rate,
        config.audio.input_gain,
        config.audio.smoothing_alpha,
        config.audio.relative_threshold_multiplier,
        config.audio.calibration_duration,
        config.audio.debounce_frames,
    )?;
    let playback = AudioPlayback::new()?;
    info!("オーディオデバイス初期化OK");

    println!();
    println!("========================================");
    println!("  Smart Speaker Ready!");
    println!("  Wakeword file: {}", config.wakeword.wakeword_path);
    println!("========================================");

    // メインループ
    loop {
        // ウェイクワード待機（Rustpotter）
        match wakeword_detector.wait_for_wakeword(&capture) {
            Ok(result) => {
                info!("ウェイクワード \"{}\" 検出 (score: {:.2})", result.keyword, result.score);

                // コマンドを録音
                println!(">>> Listening for your command...");
                match get_voice_command(&config, &capture, &stt) {
                    Ok(Some(cmd)) => {
                        // LLM応答を生成して再生
                        if let Err(e) = process_command(&cmd, &llm, &tts, &playback) {
                            error!("処理エラー: {}", e);
                        }
                    }
                    Ok(None) => {
                        warn!("コマンドを認識できませんでした。");
                    }
                    Err(e) => {
                        error!("録音エラー: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("ウェイクワード検出エラー: {}", e);
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
}

/// 音声コマンドを取得
fn get_voice_command(
    config: &Config,
    capture: &AudioCapture,
    stt: &WhisperStt,
) -> Result<Option<String>> {
    let audio_data = capture.record_with_feedback(
        config.audio.max_record_seconds,
        config.audio.silence_threshold,
        config.audio.silence_duration,
    )?;

    if audio_data.len() < (config.audio.sample_rate as usize / 2) {
        return Ok(None);
    }

    let start = std::time::Instant::now();
    info!("音声認識中...");
    let text = stt.transcribe(&audio_data)?;
    let stt_time = start.elapsed();
    info!("STT完了: {:.2}秒", stt_time.as_secs_f32());

    let text = text.trim().to_string();

    if text.is_empty() {
        return Ok(None);
    }

    println!(">>> You said: \"{}\"", text);
    Ok(Some(text))
}

/// コマンドを処理してLLM応答を生成・再生
fn process_command(
    command: &str,
    llm: &OllamaLlm,
    tts: &VoicevoxTts,
    playback: &AudioPlayback,
) -> Result<()> {
    println!(">>> Processing: \"{}\"", command);

    // LLM: テキスト→応答
    let start = std::time::Instant::now();
    info!("LLM応答生成中...");
    let response = llm.generate(command)?;
    let llm_time = start.elapsed();
    info!("LLM完了: {:.2}秒", llm_time.as_secs_f32());
    println!(">>> Response: \"{}\"", response);

    // TTS: 応答→音声
    let start = std::time::Instant::now();
    info!("音声合成中...");
    let audio_response = tts.synthesize(&response)?;
    let tts_time = start.elapsed();
    info!("TTS完了: {:.2}秒 ({} bytes)", tts_time.as_secs_f32(), audio_response.len());

    // 音声再生
    info!("応答を再生中...");
    playback.play_wav(&audio_response)?;

    println!();
    Ok(())
}
