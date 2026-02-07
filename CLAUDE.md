# Smart Speaker Project

Rust製のスマートスピーカーアプリケーション。ウェイクワード検出、音声認識、LLM応答生成、音声合成を統合したローカル動作型の音声アシスタント。

## システムアーキテクチャ

### 処理フロー

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           Smart Speaker                                  │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│   ┌─────────┐    ┌──────────────┐    ┌─────────────┐                    │
│   │  マイク  │───▶│  Rustpotter  │───▶│ ウェイクワード │                    │
│   │  入力   │    │ (Wake Word)  │    │   検出完了    │                    │
│   └─────────┘    └──────────────┘    └──────┬──────┘                    │
│                                              │                           │
│                                              ▼                           │
│                                     ┌──────────────┐                    │
│                                     │   音声録音    │                    │
│                                     │  (発話終了まで) │                    │
│                                     └──────┬──────┘                    │
│                                              │                           │
│                                              ▼                           │
│                                     ┌──────────────┐                    │
│                                     │  Whisper v3  │                    │
│                                     │    (STT)     │                    │
│                                     └──────┬──────┘                    │
│                                              │                           │
│                                              ▼                           │
│                                     ┌──────────────┐                    │
│                                     │    Ollama    │                    │
│                                     │    (LLM)     │                    │
│                                     └──────┬──────┘                    │
│                                              │                           │
│                                              ▼                           │
│                                     ┌──────────────┐                    │
│                                     │   VOICEVOX   │                    │
│                                     │    (TTS)     │                    │
│                                     └──────┬──────┘                    │
│                                              │                           │
│                                              ▼                           │
│                                     ┌──────────────┐                    │
│                                     │ スピーカー出力 │                    │
│                                     └──────────────┘                    │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

### コンポーネント概要

| コンポーネント | 役割 |
|---------------|------|
| Audio Input | マイクからの音声キャプチャ |
| Rustpotter | ウェイクワード検出（オープンソース、ローカル動作） |
| Whisper v3 | 音声からテキストへの変換（STT） |
| Ollama | テキスト応答生成（LLM） |
| VOICEVOX | テキストから音声への変換（TTS） |
| Audio Output | スピーカーへの音声再生 |

## 技術スタック

| 項目 | 技術 |
|------|------|
| 開発言語 | Rust |
| 動作環境 | Windows |
| ウェイクワード検出 | Rustpotter 3.0 |
| 音声認識 (STT) | Whisper v3 (whisper.cpp経由) |
| LLM | Ollama (モデル設定可能) |
| 音声合成 (TTS) | VOICEVOX |

## Rustpotter 設定の注意点

### 動作確認済み設定

```toml
[wakeword]
wakeword_path = "sakura.rpw"
threshold = 0.35       # 検出閾値（0.0〜1.0）
avg_threshold = 0.15   # 平均スコア閾値
min_scores = 3         # 連続検出回数
```

### 使用してはいけない設定

以下の設定は score = 0 問題を引き起こすため使用しない：

```rust
// NG: これらを設定すると検出が動作しなくなる
rustpotter_config.detector.eager = false;
rustpotter_config.filters.band_pass.enabled = true;
rustpotter_config.filters.gain_normalizer.enabled = true;
```

### ウェイクワードモデル作成

https://givimad.github.io/rustpotter-create-model-demo/ で .rpw ファイルを作成

## ディレクトリ構成

```
smart_speaker/
├── CLAUDE.md           # プロジェクトドキュメント
├── Cargo.toml          # Rust依存関係
├── config/
│   └── settings.toml   # アプリケーション設定
├── models/
│   └── ggml-*.bin      # Whisperモデルファイル
└── src/
    ├── main.rs         # エントリポイント
    ├── config.rs       # 設定読み込み
    ├── audio/
    │   ├── mod.rs
    │   ├── capture.rs  # マイク入力
    │   └── playback.rs # スピーカー出力
    ├── wakeword/
    │   ├── mod.rs
    │   └── detector.rs  # Rustpotter連携
    ├── stt/
    │   ├── mod.rs
    │   └── whisper.rs  # whisper.cpp連携
    ├── llm/
    │   ├── mod.rs
    │   └── ollama.rs   # Ollama API連携
    └── tts/
        ├── mod.rs
        └── voicevox.rs # VOICEVOX API連携
```

## 依存クレート

```toml
[dependencies]
# オーディオ入出力
cpal = "0.15"           # クロスプラットフォームオーディオ
rodio = "0.17"          # 音声再生

# ウェイクワード検出
rustpotter = "3.0"      # Rustpotter（オープンソース）

# 音声認識
whisper-rs = "0.11"     # whisper.cpp Rustバインディング

# HTTP通信
reqwest = { version = "0.11", features = ["json", "blocking"] }

# 設定・シリアライズ
serde = { version = "1.0", features = ["derive"] }
toml = "0.8"

# 非同期ランタイム
tokio = { version = "1", features = ["full"] }

# エラーハンドリング
anyhow = "1.0"
thiserror = "1.0"

# ログ
log = "0.4"
env_logger = "0.10"
```

## セットアップ手順

### 前提条件

1. **Rust** (1.70以上)
   ```powershell
   # Rustupでインストール
   winget install Rustlang.Rustup
   ```

2. **Ollama**
   ```powershell
   # Ollamaをインストール後、モデルをダウンロード
   ollama pull llama3.2
   ```

3. **VOICEVOX**
   - [VOICEVOX公式サイト](https://voicevox.hiroshiba.jp/)からダウンロード・インストール
   - デフォルトでhttp://localhost:50021で起動

4. **Picovoice Access Key**
   - [Picovoice Console](https://console.picovoice.ai/)でアカウント作成
   - Access Keyを取得

5. **Whisperモデル**
   - [whisper.cpp](https://github.com/ggerganov/whisper.cpp)からggml形式モデルをダウンロード
   - `models/`ディレクトリに配置

### ビルド・実行

```powershell
# ビルド
cargo build --release

# 実行
cargo run --release
```

## 設定ファイル仕様

`config/settings.toml`:

```toml
[general]
# ログレベル: trace, debug, info, warn, error
log_level = "info"

[wakeword]
# Picovoice Access Key
access_key = "YOUR_PICOVOICE_ACCESS_KEY"
# ウェイクワードキーワードファイルパス（.ppnファイル）
keyword_path = "path/to/keyword.ppn"
# 検出感度 (0.0 - 1.0)
sensitivity = 0.5

[stt]
# Whisperモデルパス
model_path = "models/ggml-large-v3.bin"
# 言語設定
language = "ja"

[llm]
# OllamaエンドポイントURL
endpoint = "http://localhost:11434"
# 使用するモデル名
model = "llama3.2"
# システムプロンプト
system_prompt = "あなたは親切なアシスタントです。3文以内で簡潔に日本語で回答してください。"

[tts]
# VOICEVOXエンドポイントURL
endpoint = "http://localhost:50021"
# 話者ID (0: 四国めたん, 1: ずんだもん, etc.)
speaker_id = 1
# 話速 (0.5 - 2.0)
speed = 1.0

[audio]
# サンプルレート
sample_rate = 16000
# 録音最大時間（秒）
max_record_seconds = 10
# 無音検出閾値
silence_threshold = 0.01
# 無音継続時間で録音終了（秒）
silence_duration = 1.5
```

## 外部サービスAPI

### Ollama API

```
POST http://localhost:11434/api/generate
Content-Type: application/json

{
  "model": "llama3.2",
  "prompt": "こんにちは",
  "stream": false
}
```

### VOICEVOX API

```
# 1. 音声合成用クエリ作成
POST http://localhost:50021/audio_query?text=こんにちは&speaker=1

# 2. 音声合成
POST http://localhost:50021/synthesis?speaker=1
Content-Type: application/json
Body: (audio_queryのレスポンス)
```

## 開発コマンド

```powershell
# フォーマット
cargo fmt

# Lint
cargo clippy

# テスト
cargo test

# ドキュメント生成
cargo doc --open
```
