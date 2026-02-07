# Smart Speaker Debug Session - 2026-01-30

## Overview

This document summarizes the debugging session for the Smart Speaker wakeword detection system using Rustpotter.

---

## Problem 1: Wakeword File Path Configuration

### Symptom
- Application could not find the wakeword file

### Root Cause
- `config/settings.toml` was pointing to a non-existent file: `models/wakeword.rpw`
- The actual wakeword file `sakura.rpw` was located in the project root

### Solution
```toml
# Before
wakeword_path = "models/wakeword.rpw"

# After
wakeword_path = "sakura.rpw"
```

**File**: `config/settings.toml`

---

## Problem 2: No Audio Data from Microphone (RMS = 0)

### Symptom
- Debug output showed `max_val=0.0000000006` (essentially zero)
- `rms=0.0000` continuously displayed
- Microphone was working in Windows Sound Settings

### Investigation Steps
1. Added debug output to display available input devices
2. Confirmed device was detected: `Realtek(R) Audio`
3. Confirmed Windows was using 48000Hz, 2ch (stereo)
4. Target sample rate was 16000Hz

### Root Cause
- **Sample rate mismatch**: Device was 48kHz, but code was requesting 16kHz samples without proper conversion
- The `record_samples` method was requesting `num_samples` (480) at device rate (48kHz) instead of calculating the equivalent samples needed

### Solution
Modified `src/audio/capture.rs` to calculate device samples based on sample rate ratio:

```rust
// Before: Requested 480 samples at 48kHz (wrong)
let target_mono_samples = num_samples;

// After: Calculate samples needed at device rate
let device_samples = if self.sample_rate != self.target_sample_rate {
    ((num_samples as f64) * (self.sample_rate as f64) / (self.target_sample_rate as f64)).ceil() as usize
} else {
    num_samples
};
```

**Result**: `max_val` changed from `0.0000000006` to `0.02~0.04` (normal audio levels)

---

## Problem 3: Wakeword Detection Not Working

### Symptom
- Audio data was being captured correctly (rms > 0)
- But wakeword was never detected, or detected on any sound

### Investigation Steps
1. Examined Rustpotter source code in `~/.cargo/registry/src/.../rustpotter-3.0.2/`
2. Found critical code in `detector.rs:249-250`:
   ```rust
   if audio_samples.len() != self.get_samples_per_frame() {
       return None;
   }
   ```
3. Discovered `RustpotterConfig` was using default `sample_format: F32` while code was sending `i16`

### Root Causes
1. **Sample format mismatch**: RustpotterConfig defaulted to F32, but code was sending i16
2. **Sample count mismatch**: Resampling could produce slightly different sample counts due to floating-point precision
3. **Missing explicit configuration**: RustpotterConfig was not explicitly set for the audio format being used

### Solution

#### 1. Explicit RustpotterConfig setup (`src/wakeword/detector.rs`)
```rust
// Before
let mut rustpotter_config = RustpotterConfig::default();
rustpotter_config.detector.threshold = config.threshold;
rustpotter_config.detector.avg_threshold = config.avg_threshold;

// After
let mut rustpotter_config = RustpotterConfig::default();

// Explicitly set audio format (16kHz, mono, i16)
rustpotter_config.fmt.sample_format = SampleFormat::I16;
rustpotter_config.fmt.sample_rate = 16000;
rustpotter_config.fmt.channels = 1;

rustpotter_config.detector.threshold = config.threshold;
rustpotter_config.detector.avg_threshold = config.avg_threshold;
```

#### 2. Ensure exact sample count after resampling (`src/audio/capture.rs`)
```rust
// Before
Ok(i16_samples)

// After
// Ensure exact sample count matches what Rustpotter expects
i16_samples.resize(num_samples, 0);
Ok(i16_samples)
```

---

## Summary of Changes

| File | Change |
|------|--------|
| `config/settings.toml` | Fixed wakeword_path to `sakura.rpw` |
| `src/audio/capture.rs` | Calculate device samples based on sample rate ratio |
| `src/audio/capture.rs` | Resize output to exact sample count |
| `src/wakeword/detector.rs` | Import `SampleFormat` |
| `src/wakeword/detector.rs` | Explicitly configure RustpotterConfig (16kHz, mono, i16) |

---

## Key Learnings

1. **Sample Rate Conversion**: When device sample rate differs from target, calculate equivalent samples:
   ```
   device_samples = target_samples * (device_rate / target_rate)
   ```

2. **Rustpotter Requirements**:
   - Expects **exactly** `get_samples_per_frame()` samples (returns `None` otherwise)
   - Default sample rate is 16000Hz
   - Supports i8, i16, i32, f32 formats via `Sample` trait
   - Configuration must match the actual audio format being sent

3. **Debugging Audio Issues**:
   - Add RMS/max_val debug output to verify audio capture
   - Check sample counts at each stage of the pipeline
   - Verify device configuration (sample rate, channels, format)

---

## Configuration Reference

### settings.toml
```toml
[wakeword]
wakeword_path = "sakura.rpw"
threshold = 0.3        # Detection threshold (0.0-1.0)
avg_threshold = 0.1    # Average score threshold

[audio]
sample_rate = 16000    # Target sample rate for Rustpotter
```

### Audio Pipeline
```
Microphone (48kHz, stereo, f32)
    ↓ stereo → mono (average channels)
    ↓ collect device_samples (1440 samples at 48kHz)
    ↓ resample to 16kHz (480 samples)
    ↓ f32 → i16 conversion (* 32767)
    ↓ resize to exact num_samples
Rustpotter (16kHz, mono, i16)
```
