whisper-rs 精度改善 実装チェックリスト
目的

ローカル音声認識（whisper-rs）において認識精度を最大化する。
Whisperは モデルサイズよりも前処理品質の影響が大きい ため、音声パイプラインの最適化を最優先とする。

必須（これが未実装だと精度が大きく低下）
1. モノラル化（stereo禁止）

Whisper入力仕様は mono 固定。

要件

入力チャンネル数は 1ch

実装例
mono = (left + right) * 0.5;

2. サンプリングレート 16kHz 固定

Whisper内部は 16kHz 前提学習。

要件

16,000 Hz に変換すること

44.1kHz / 48kHz をそのまま渡さない

推奨（軽量）
// 48kHz -> 16kHz
samples.iter().step_by(3)

高品質（必要な場合のみ）

rubato などで resample

3. 音量正規化（最重要）

入力振幅不足は精度低下の最大要因。

要件

ピーク振幅を 0.8〜0.95 に正規化

実装例
let max = samples.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
if max > 0.0 {
    let gain = 0.9 / max;
    for s in &mut samples {
        *s *= gain;
    }
}

4. f32形式で入力

whisper-rs は f32 を期待。

要件

range: -1.0 〜 1.0

i16のまま渡さない

変換例
let f32_samples: Vec<f32> =
    i16_samples.iter().map(|s| *s as f32 / 32768.0).collect();

強く推奨（精度がさらに向上）
5. VAD（無音除去）

無音区間は誤認識の原因。

対策候補

webrtcvad

silero-vad

whisper.cpp互換VAD

要件

無音区間は推論しない

音声区間のみ Whisper に渡す

6. 言語固定（auto禁止）

自動言語判定は誤判定の原因。

実装例
params.set_language(Some("ja")); // or "en"

7. モデルサイズ選択

tiny/base は精度不足になりやすい。

推奨
用途	モデル
軽量リアルタイム	small
高精度	medium
8. temperature = 0

ランダム性排除で安定化。

params.set_temperature(0.0);

9. beam search 有効化（オフライン用途）

精度↑ 速度↓

params.set_beam_size(5);

任意（環境依存）
10. 低周波ノイズ除去（HPF）

空調・振動ノイズ対策。

推奨

80Hz以下カット

11. バッファサイズ最適化

大きすぎると遅延増加。

推奨
10〜30ms (160〜480 samples @16kHz)

推奨パイプライン（最終形）
cpal (48kHz stereo f32)
      ↓
mono化
      ↓
16kHz ダウンサンプル
      ↓
正規化
      ↓
VAD
      ↓
whisper-rs (f32 mono 16kHz)

whisper-rs 推奨設定サンプル
let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5 });

params.set_language(Some("ja"));
params.set_temperature(0.0);
params.set_print_progress(false);
params.set_print_special(false);
params.set_print_realtime(false);
params.set_print_timestamps(false);

精度改善の優先度（重要度順）

音量正規化

16kHz変換

mono化

モデル small以上

VAD

言語固定

beam search

備考

wake word用途と違い、Whisperは音量とノイズの影響が非常に大きい

「モデルを大きくする前に前処理を整える」こと