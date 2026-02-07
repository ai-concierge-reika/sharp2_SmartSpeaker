rustpotter Wake Word 精度改善 実装チェックリスト
目的

rustpotter を使用した wake word 検出において、
誤検出・未検出・反応遅延を最小化し、リアルタイム性と精度を最大化する。

注意
wake word 検出はモデル品質よりも「音声前処理・入力フォーマット」が支配的。

必須（未実装だとほぼ確実に失敗）
1. モノラル化（mono固定）

rustpotterは mono 前提。

要件

1chのみ入力

stereo禁止

実装例
mono = (left + right) * 0.5;

2. サンプリングレート 16kHz 固定（最重要）

rustpotter内部は 16kHz 前提。

NG

44.1kHz / 48kHz をそのまま入力

OK

16,000 Hz に変換

軽量ダウンサンプル（推奨）
// 48kHz → 16kHz
samples.iter().step_by(3)

高品質（必要時のみ）

rubato / speex resampler

※ wake word用途では step_by で十分

3. i16 フォーマット必須

rustpotterは i16 前提。

要件

signed 16bit

range: -32768〜32767

変換例
let i16_samples: Vec<i16> =
    f32_samples.iter().map(|s| (s * 32767.0) as i16).collect();

注意（超重要）
sample as i16   // ❌ 絶対NG


→ ほぼ無音になる

4. 音量正規化（非常に重要）

入力振幅不足は未検出の最大原因。

要件

ピーク振幅 70〜90% 程度

実装例
let max = samples.iter().map(|s| s.abs()).max().unwrap_or(1) as f32;
let gain = 28000.0 / max;

for s in &mut samples {
    *s = (*s as f32 * gain) as i16;
}

強く推奨（精度と安定性が向上）
5. バッファサイズ最適化（遅延対策）

バッファが大きいと検出遅延増加。

推奨
160〜480 samples @ 16kHz
= 10〜30ms

cpal例
BufferSize::Fixed(256)

6. 連続フレーム供給（途切れ禁止）

rustpotterはストリーミング前提。

要件

無音でも常時 feed する

欠損フレーム禁止

NG

音がある時だけ入力

7. 軽量VAD（誤検出削減）

ノイズや環境音による誤トリガ防止。

対策候補

webrtcvad

silero-vad

方針

無音区間はrustpotterに送らない or ゲイン低減

8. マイクゲイン設定（OS側）

小声は検出不能になりやすい。

Windows推奨
入力ボリューム 80〜100%
AGC OFF（可能なら）

モデル関連（必要に応じて）
9. 閾値 (threshold) 調整

未検出/誤検出のトレードオフ。

目安
threshold	挙動
低い	誤検出増
高い	未検出増
推奨開始値
0.5〜0.7

10. 検出クールダウン

連続誤検出防止。

実装例
検出後 1〜2秒は再検出禁止

11. 入力ノイズ除去（任意）

低周波ノイズは誤検出原因。

推奨

80Hz以下ハイパス

RNNoise（必要なら）

推奨パイプライン（完成形）
cpal (48kHz stereo f32)
      ↓
mono化
      ↓
16kHz ダウンサンプル
      ↓
正規化
      ↓
f32 → i16
      ↓
VAD
      ↓
rustpotter.feed()

実装テンプレ（最小例）
fn preprocess(input: &[f32]) -> Vec<i16> {
    // mono + downsample
    let mono: Vec<f32> = input
        .chunks(2)
        .map(|c| (c[0] + c[1]) * 0.5)
        .step_by(3)
        .collect();

    // normalize
    let max = mono.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
    let gain = if max > 0.0 { 0.9 / max } else { 1.0 };

    mono.iter()
        .map(|s| (s * gain * 32767.0) as i16)
        .collect()
}

精度改善 優先度（重要度順）

16kHz変換

音量正規化

mono化

i16変換スケール

バッファ小型化

threshold調整

VAD

ノイズ除去

備考

wake wordは音質より「安定した振幅・低遅延」が重要

高品質リサンプラより step_by の方が実用的

「常時入力」「小さなフレーム」「十分な音量」が成功の鍵