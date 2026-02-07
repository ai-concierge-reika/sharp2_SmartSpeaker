use anyhow::Result;
use log::{debug, info};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::io::Cursor;
use thiserror::Error;

/// 音声再生に関するエラー
#[derive(Debug, Error)]
pub enum PlaybackError {
    #[error("出力デバイスの初期化に失敗: {0}")]
    DeviceError(String),

    #[error("音声データのデコードに失敗: {0}")]
    DecodeError(String),

    #[error("再生中にエラーが発生: {0}")]
    PlayError(String),
}

/// スピーカーへの音声再生を管理
pub struct AudioPlayback {
    _stream: OutputStream,
    handle: OutputStreamHandle,
}

impl AudioPlayback {
    /// デフォルトの出力デバイスでAudioPlaybackを初期化
    pub fn new() -> Result<Self> {
        let (stream, handle) = OutputStream::try_default()
            .map_err(|e| PlaybackError::DeviceError(e.to_string()))?;

        info!("音声再生デバイスを初期化しました");

        Ok(Self {
            _stream: stream,
            handle,
        })
    }

    /// WAV形式の音声データを再生（再生完了まで待機）
    ///
    /// # Arguments
    /// * `wav_data` - WAV形式の音声データ（バイト列）
    pub fn play_wav(&self, wav_data: &[u8]) -> Result<()> {
        debug!("WAV再生開始: {} bytes", wav_data.len());

        let cursor = Cursor::new(wav_data.to_vec());
        let source = Decoder::new(cursor)
            .map_err(|e| PlaybackError::DecodeError(e.to_string()))?;

        let sink = Sink::try_new(&self.handle)
            .map_err(|e| PlaybackError::PlayError(e.to_string()))?;

        sink.append(source);
        sink.sleep_until_end();

        debug!("WAV再生完了");
        Ok(())
    }

    /// WAV形式の音声データを非同期で再生（待機なし）
    ///
    /// # Arguments
    /// * `wav_data` - WAV形式の音声データ（バイト列）
    ///
    /// # Returns
    /// 再生を制御するためのSink
    pub fn play_wav_async(&self, wav_data: &[u8]) -> Result<Sink> {
        debug!("WAV非同期再生開始: {} bytes", wav_data.len());

        let cursor = Cursor::new(wav_data.to_vec());
        let source = Decoder::new(cursor)
            .map_err(|e| PlaybackError::DecodeError(e.to_string()))?;

        let sink = Sink::try_new(&self.handle)
            .map_err(|e| PlaybackError::PlayError(e.to_string()))?;

        sink.append(source);

        Ok(sink)
    }
}
