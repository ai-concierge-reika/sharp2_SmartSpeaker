# Smart Speaker Debug Session - 2026-01-31

## Overview

Rustpotter wake word detection was not triggering at all. Investigation revealed a critical bug in the audio streaming architecture where the ring buffer was feeding duplicate/overlapping audio frames to the detector instead of sequential frames.

---

## Problem 1: Ring Buffer Reading Overlapping Audio Frames

### Symptom
- Wake word never detected despite speaking clearly
- `score` value in status line remained at 0 or very low
- Detection loop appeared to run but never triggered

### Investigation Steps

1. **Traced audio flow**: `AudioCapture` → ring buffer → `record_samples()` → `WakewordDetector`

2. **Examined `read_latest()` method** in `capture.rs:59-76`:
   ```rust
   // Always reads from the most recent write position
   let start_pos = (self.write_pos + RING_BUFFER_CAPACITY - actual_samples) % RING_BUFFER_CAPACITY;
   ```

3. **Identified the problem**: Each call to `record_samples()` returned the **latest N samples**, not the **next N samples** in sequence

4. **Confirmed impact**: Rustpotter's detection algorithm expects consecutive, non-overlapping audio frames. Feeding it duplicate data corrupts the internal state.

### Root Cause

The ring buffer implementation had no concept of a "read position" for streaming. It only tracked `write_pos` and `total_written`, always returning the most recent samples regardless of what was previously read.

**Behavior:**
- If detector ran faster than audio arrived → Same audio processed multiple times
- If detector ran slower → Audio frames skipped entirely
- Both scenarios break Rustpotter's pattern matching

### Solution

Added streaming read support with a dedicated read position tracker.

**Before** (`capture.rs`):
```rust
struct AudioCaptureInner {
    ring_buffer: Vec<f32>,
    write_pos: usize,
    total_written: u64,
}

fn read_latest(&self, num_samples: usize) -> Vec<f32> {
    // Always reads most recent samples - WRONG for streaming
    let start_pos = (self.write_pos + RING_BUFFER_CAPACITY - actual_samples) % RING_BUFFER_CAPACITY;
    // ...
}
```

**After** (`capture.rs`):
```rust
struct AudioCaptureInner {
    ring_buffer: Vec<f32>,
    write_pos: usize,
    total_written: u64,
    stream_read_pos: u64,  // NEW: tracks where we last read
}

fn unread_samples(&self) -> usize {
    if self.total_written <= self.stream_read_pos {
        return 0;
    }
    let unread = self.total_written - self.stream_read_pos;
    unread.min(RING_BUFFER_CAPACITY as u64) as usize
}

fn read_stream(&mut self, num_samples: usize) -> Vec<f32> {
    let available = self.unread_samples();
    let to_read = num_samples.min(available);
    // ... read from stream_read_pos, then advance it
    self.stream_read_pos += to_read as u64;
    result
}
```

**Files Modified**: `src/audio/capture.rs`

---

## Problem 2: Insufficient Diagnostic Output

### Symptom
- Difficult to determine if audio was being captured
- No visibility into whether model was recognizing partial patterns
- Hard to tell if thresholds were too strict

### Investigation Steps
1. Reviewed existing debug output in `detector.rs:124-129`
2. Noted that partial score was shown but no historical maximum
3. Users couldn't tell if model was "close" to detecting

### Root Cause
Status line only showed current frame values, making it hard to see if the model ever responded to voice input.

### Solution

Added max value tracking for both RMS and detection score.

**Before** (`detector.rs`):
```rust
print!(
    "\r  [Listening] rms:{:.4} score:{:.3} samples:[{},{}]    ",
    rms, partial_score, sample_min, sample_max
);
```

**After** (`detector.rs`):
```rust
let mut max_rms_seen: f32 = 0.0;
let mut max_score_seen: f32 = 0.0;

// In loop:
if rms > max_rms_seen { max_rms_seen = rms; }
if partial_score > max_score_seen { max_score_seen = partial_score; }

print!(
    "\r  [Listening] rms:{:.4} (max:{:.4}) score:{:.3} (max:{:.3}) amp:[{},{}]    ",
    rms, max_rms_seen, partial_score, max_score_seen, sample_min, sample_max
);
```

**File**: `src/wakeword/detector.rs`

---

## Problem 3: No Model File Validation

### Symptom
- If `.rpw` file path was wrong, error message was unclear
- Relative path resolution issues went undetected

### Investigation Steps
1. Checked `settings.toml`: `wakeword_path = "sakura.rpw"` (relative path)
2. Noted no existence check before loading

### Root Cause
Rustpotter's error message when file doesn't exist is generic. No pre-validation was performed.

### Solution

Added explicit file existence check with helpful error message.

**Before** (`detector.rs`):
```rust
pub fn new(config: &WakewordConfig) -> Result<Self> {
    let mut rustpotter_config = RustpotterConfig::default();
    // ... directly proceeded to load
}
```

**After** (`detector.rs`):
```rust
pub fn new(config: &WakewordConfig) -> Result<Self> {
    // Model file existence check
    let wakeword_path = std::path::Path::new(&config.wakeword_path);
    if !wakeword_path.exists() {
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
    // ... continue with initialization
}
```

**File**: `src/wakeword/detector.rs`

---

## Problem 4: Stream Position Not Reset Between Detection Sessions

### Symptom
- After wake word detected and command processed, next detection session might have stale data

### Root Cause
When `wait_for_wakeword()` is called again after processing a command, the stream read position pointed to old data.

### Solution

Reset stream position at start of each detection session.

```rust
pub fn wait_for_wakeword(&mut self, capture: &AudioCapture) -> Result<WakewordResult> {
    // Reset stream position for fresh sequential reads
    capture.reset_stream_position();
    debug!("ストリーム読み取り位置をリセット");
    // ...
}
```

**Files**: `src/audio/capture.rs` (added `reset_stream_position()` method), `src/wakeword/detector.rs` (calls it)

---

## Summary of Changes

| File | Change |
|------|--------|
| `src/audio/capture.rs` | Added `stream_read_pos` field, `unread_samples()`, `read_stream()`, `reset_stream_position()` methods; rewrote `record_samples()` to use streaming |
| `src/wakeword/detector.rs` | Added model file validation, stream position reset, max value tracking in status output |

---

## Key Learnings

1. **Ring buffers for streaming need read position tracking** - A ring buffer that only tracks write position works for "get latest" scenarios but fails for continuous streaming where each consumer needs sequential, non-overlapping data.

2. **Audio detection algorithms are stateful** - Rustpotter (and similar wake word engines) maintain internal state across frames. Feeding duplicate or out-of-order frames corrupts this state and prevents detection.

3. **Show maximums in diagnostic output** - When debugging real-time systems, showing "max value seen" helps determine if the system ever responded correctly, even briefly.

4. **Validate external resources early** - Check file existence before passing to libraries. Library error messages are often less helpful than custom validation.

---

## Configuration Reference

### 動作確認済みの設定（2026-01-31）

```toml
# config/settings.toml

[wakeword]
wakeword_path = "sakura.rpw"
threshold = 0.35       # 検出閾値
avg_threshold = 0.15   # 平均スコア閾値
min_scores = 3         # 連続検出回数
```

**重要な注意点：**
- `eager = false` を設定すると score が 0 になる問題あり → 設定しない
- バンドパスフィルタ、ゲイン正規化も score = 0 を引き起こす可能性あり → 使用しない
- Rustpotter のデフォルト設定をできるだけ維持する

### Rustpotter 設定（detector.rs）

```rust
// 動作する最小限の設定
rustpotter_config.fmt.sample_format = SampleFormat::I16;
rustpotter_config.fmt.sample_rate = 16000;
rustpotter_config.fmt.channels = 1;

rustpotter_config.detector.threshold = config.threshold;
rustpotter_config.detector.avg_threshold = config.avg_threshold;
rustpotter_config.detector.min_scores = config.min_scores;

// 以下は設定しない（score = 0 問題を引き起こす）
// rustpotter_config.detector.eager = false;
// rustpotter_config.filters.band_pass.enabled = true;
// rustpotter_config.filters.gain_normalizer.enabled = true;
```

### Diagnostic Output Interpretation

```
[Listening] rms:0.0123 (max:0.0456) score:0.150 (max:0.320) amp:[-1234,5678]
            │          │            │           │           │
            │          │            │           │           └── Sample amplitude range (i16)
            │          │            │           └── Highest score seen this session
            │          │            └── Current partial detection score
            │          └── Highest RMS seen this session
            └── Current audio level (0 = silence)
```

**Healthy values:**
- `rms > 0.01` when speaking → Microphone working
- `amp` values varying → Audio contains signal (not zeros)
- `score` increases when saying wakeword → Model recognizing patterns
- `max score` approaches threshold → Close to detection

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                    Audio Capture Flow (Fixed)                    │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   ┌──────────┐    ┌─────────────────────────────────────────┐   │
│   │ CPAL     │    │           Ring Buffer                    │   │
│   │ Callback │───▶│  [────────────────────────────────────]  │   │
│   │ (48kHz)  │    │   ↑                              ↑       │   │
│   └──────────┘    │   write_pos              stream_read_pos │   │
│                   │   (continuous)           (tracks reads)  │   │
│                   └──────────────────────────────────────────┘   │
│                                      │                           │
│                                      │ read_stream()             │
│                                      │ (sequential, no overlap)  │
│                                      ▼                           │
│                   ┌─────────────────────────────────────────┐   │
│                   │  Resample 48kHz → 16kHz                  │   │
│                   │  Convert f32 → i16                       │   │
│                   └──────────────────────────────────────────┘   │
│                                      │                           │
│                                      ▼                           │
│                   ┌─────────────────────────────────────────┐   │
│                   │  Rustpotter.process_samples()            │   │
│                   │  (expects consecutive frames)            │   │
│                   └──────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```
