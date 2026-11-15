# ADR 0369: 素材の有限性とゲインの積を、エンジンの入口の型で確かめる

日付: 2026-11-15
状態: 採用。
関連: [ADR 0359](0359-finite-overflow-is-a-fault.md)（「残る範囲」の 1 行目を閉じる）、[ADR 0241](0241-unscaled-placement-pcm.md)（桁あふれの判定の所在を移す）、
[ADR 0273](0273-shared-source-clip-windows.md)、[ADR 0108](0108-phase1-loop-readiness.md)、[ADR 0276](0276-priority-order-and-delegated-decisions.md)、
[監査の NFA-04](../reviews/2026-11-13-finite-overflow-audit.md#nfa-04-素材の不変条件がエンジンの入口にない)

## 問題と再現

開始時の HEAD は `f5fc329`。
標本の有限性と、窓のピークとゲインの積の有限性は、`echode-audio-io/src/clips.rs` の `Planes` と `prepare_media` だけが確かめていた。
エンジンの `Prepared::new` は `clip_gain_allowed`・左右の長さ・トラックの有無だけを見て、`set_cached_tracks` は標本を見なかった。
試聴の `AuditionSound::new` も、同じ有限性と長さの規則を別の論理で持っていた。
同じ規則を層ごとに別の論理で実装していた（ADR 0276）。

製品の読込み（機器の再生、CLI・GUI の書出し）は `prepare_media` を通るので、利用者は無限大を作るクリップへ届かなかった。
届くのは `set_clip_loader`・`render_offline_blocks_with_clips`・`set_cached_tracks` を直接呼ぶ経路で、今はテストと、Phase 3 のレンダリングキャッシュの予定である。
利用者に届く現れは別にあった。素材のピークと大きなクリップゲイン（モデルは 2^20 まで許す）の積があふれる編集は確定し、
その後の読込みが `PlaybackError::Media`（「audio file could not be read」）で断られた。変換後の面が有限でない時も同じ `Media` だった。

監査の再現 3 件を HEAD の作業ツリーで `--ignored` で実行し、監査の書いた理由で失敗することを確かめた（どれも終了コード 101）。

- `audit_nfa04_the_engine_refuses_a_clip_whose_gain_overflows_its_samples`: `the engine accepted the clip: Err(Render(NonFiniteMaster { at: 0 }))`。
- `audit_nfa04_the_engine_refuses_a_cached_track_with_a_nan`: `a cache with a NaN was staged`。
- 実ヘルパーの `audit_nfa04_an_effect_never_receives_an_infinity_from_placed_audio`: `the effect was given an infinity and named for returning it`。

音声機器・アプリの操作は使っていない。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | `Prepared::new` と `set_cached_tracks` が毎回全標本を走査する | 採らない。計画の準備は編集・ループ変更ごとに走り、素材は保持して再利用する。1 時間の 48 kHz ステレオは 3.5 億標本で、毎回の走査は準備を遅らせ、ADR 0241 が避けた PCM の再走査を戻す |
| 2 | 検査の関数を公開し、ローダーとキャッシュの供給側に呼ばせる | 採らない。呼ばない呼び手が残り、入口は何も保証しない |
| 3 | 作る時に 1 回だけ走査してピークを持つ型を、`LoadedClip` と `CachedTrack` に持たせる。入口はピークとゲインの積だけを見る | 採る。監査の案。型を作れない値はエンジンへ渡せない。走査の回数は従来の `echode-audio-io` と同じか少ない |

## 決定

- `echode-engine` の `planes.rs` に、確かめた面の型を置く。
  - `FinitePlanes`: モノか同じ長さの左右で、全標本が有限。作る時に左右の最大の絶対値を記録する。
    作れるのは `FinitePlanes::new` だけで、`PlaneFault::Lengths` か `PlaneFault::NonFinite { frame }` で断る。位置は左右で早い方である。
  - `PlacedPlanes`: `FinitePlanes`、配置が読む `ClipWindow`、その範囲の最大の絶対値。
    `whole` は標本を読まない。`windowed` は窓の範囲を 1 回だけ読み、面と同じ窓は `whole` と同じにする（`window()` が `None`）。
    トリムの外の有限の極大値は、その配置のゲインを制限しない（ADR 0241 の規則をここへ移した）。
  - `ClipWindow` もここへ移し、公開の名前は変えない。
- `LoadedClip` は `audio: PlacedPlanes` を持ち、`samples`・`right`・`window` を持たない。`LoadedClip::new` は単位ゲイン・フェードなしで作る。
  `CachedTrack` は `planes: FinitePlanes` を持つ。どちらも、確かめていない標本を持てない。
- `Prepared::new` は配置ごとに `PlacedPlanes::allows_gain`（読む範囲のピーク × ゲインが有限）を調べ、
  断る時は `PlaybackError::ClipGainOverflow { track, start_sample }` を返す。PCM は走査しない。
  モノの昇格と端のフェードは 1 以下の係数なので、その後に無限大は生じない（`voice::faded_sample` の順）。
- `echode-audio-io` は `Planes`・`window_peak`・ゲインの判定と、`placed_plane` の複写中の有限性の検査を削り、エンジンの型を作る。
  変換後の面が有限でない時は `PlaybackError::Planes(PlaneFault)` で、保存済み PCM が読めない・壊れている `Media` と分ける。
- `AuditionSound::new` も `FinitePlanes::new` で同じ規則を使う。非有限値と長さの違いは `Planes`、空の音は従来どおり `Media` である。
- 保存形式、モデルの上限、音を作る計算の順と丸め、音声スレッドの処理は変えない。

### 失敗した後の準備の保持

ADR 0241 は「失敗しても最後に成功した準備を失わない」とした。判定がローダーから入口へ移ったので、
あふれるゲインでもローダーの準備は成功として保持され、窓と PCM は次の読込みで再利用される。
エンジンは入口で断り、前の計画を鳴らし続ける。読込みの失敗と同じく `preparation_error` へ入る。
ゲインを戻す編集は PCM を読み直さない。`cached_window_peaks_reject_gain_overflow_without_rereading_pcm` は、
素材を消した後にゲイン 2 が入口で断られ、0・0.5・1 とトリム後のゲイン 2 が入口を通ることを確かめる。

### 実時間の制約と走査の回数

- 音声スレッドの処理は変えない。入口の判定は計画の準備（制御側）で配置ごとに 1 回の掛け算である。
- 標本の走査は面を作る時だけである。素材の面は従来の `Planes::new` と同じ 1 回、共有する窓は従来の `window_peak` と同じ 1 回である。
  複写する短いトリムは、従来は複写中と `Planes::new` の 2 回だったのが、`FinitePlanes::new` の 1 回になった。時間は測っていない。

## 回帰と基盤

- 監査の再現 3 件の `#[ignore]` を外した。
  - クリップは、書出しと機器の `replace` の両方で `ClipGainOverflow` が配置を名指し、ブロックを 1 つも描かないこと、単位ゲインの同じ素材は鳴ることを確かめる。
  - キャッシュは、型が NaN を拒否するので、期待を「入口で断る」から「作れない」へ改め、名前を `audit_nfa04_a_cached_track_cannot_hold_a_nan` にした。
  - 実ヘルパーは、入口で断ること、エフェクトが原因として記録されないこと、前の計画が無音のまま鳴ることを確かめる。
- `planes_tests.rs` は、型の拒否と最初の位置、窓のピーク（負の先行・面の外・面と同じ窓）、窓の読み方を確かめる。
- CLI の `an_export_refuses_a_clip_gain_its_loudest_sample_overflows` を必須契約 `clip-gain-entrance` にした（`scripts/verification.json`）。
  実取込みの 3.0e38 の float WAV にゲイン 2 を付けた書出しを、配置を名指す型付きの拒否にし、既存の書出し先を保全する。単位ゲインの同じ素材は書き出せる。
  陰性対照（入口の判定を外す）では、`master: audio sum produced a NaN or an infinity at sample 0` で失敗することを確かめた。
- 既存の回帰（`block_size` の窓・ゲイン、`clips_tests` の窓の共有と再利用）は、エンジンの型で同じ面を作る形へ書き換え、期待値は変えていない。
- 設計書 §7.5、デバッグの手順、開発手順のコードの所在、監査の表、統合表、ADR 0359・0241 の参照を更新した。

## 残る範囲

- 範囲外のクリップゲインと、計画にないトラックへの配置は、従来どおり `Media` で断る。モデルの不変条件が先に止めるので、製品の経路では届かない。
- 試聴の先読みリング（`AuditionWriter::push`）は、面を持たない逐次の書込みなので、フレームごとに有限性を調べ、型を使わない。
- 公開の `render_faded_clips`・`render_audio_clips` は任意のスライスを受け、型を通らない。製品の経路では使わない。
- キャッシュの供給側（Phase 3）は未実装で、`set_cached_tracks` は製品から呼ばれない（ENG-A1）。
- GUI は準備の失敗を従来の経路で示す。ゲインのあふれの文言が変わったことは、実画面で確かめていない。
