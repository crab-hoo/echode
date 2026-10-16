# 2026-10-17 境界の再監査

静的な指摘 5 件を、実装と既存の ADR に照らして確認した。指摘側が実行したのは、リポジトリ状態と対象一覧の読み取りだけである。テスト、ファザー、ビルドは指摘側では実行していない。

tick の単位だけは指摘の前提と製品が違う。指摘は 960 PPQ でサンプル数を書いている。Echode の 1 拍は `TICKS_PER_QUARTER = 705_600` である。区間の取り違え自体は、この分解能でも残る。120 BPM の 2 拍は 48 kHz で 48_000 サンプル、その後の 60 BPM の同じ tick 幅は 96_000 サンプルになる。

## 採用した指摘

1. 負の `Placement::offset` は不変条件を満たす。再生と書き出しは `start + offset` をサンプル位置にして `PlaybackError::Time` にしていた。波形は負のフレームを 0 にクランプし、無音であるべき区間までファイル先頭を引き伸ばしていた。
2. 内容位置 0 は `start - offset` に固定される。スキップするメディアは `[start - offset, start]` のテンポ積分である。実装は `[start, start + offset]` を積分していた。テンポ変化で発音位置がずれ、一定テンポでも非整数サンプル/tick では最大 1 サンプルずれる。
3. バックグラウンドの `commit_plugin_state` が通常の `perform` を呼び、Redo を消し、Undo に積んでいた。Swift の Undo / Redo 数は画面操作の成功時だけ変わるため、ボタンと実行結果がずれる。取り込み後に音声側のプロジェクトを更新しないと、次の履歴操作でグラフを作り直していた。
4. クリップゲインは有限かつ 0 以上だけで、`f32::MAX` を許していた。トラックのフェーダー上限は 4 である。大きなクリップゲインはサンプルを無限大にし、出力の段差補正が NaN を作る。ADR 0107 はゲイン 1000 を有効なモデル値として扱うため、上限を 4 にはしていない。
5. キャッシュ読出は `RenderRange` を見ていなかった。停止テールでは未開始の領域が鳴り、ループ終端を超えて読んでいた。

## 対応

判断は [ADR 0109](../decisions/0109-placement-bounds-and-plugin-capture.md)。負のオフセットはファイル前の無音にする。プラグインの自動保存は Undo に積まず、ジャーナルと自動保存には残す。クリップゲインの上限は 1_048_576 とし、非有限の出力サンプルは 0 にする。停止中のキャッシュは未開始なら無音、終端はループ終端で切る。

すでに鳴り始めた平坦なキャッシュの中の、停止位置より後の発音は分離しない。供給側がその内容を平面へ入れない。Phase 3 のキャッシュ生成は、この対応では接続していない。

## 回帰

- `placement_lead_uses_the_interval_before_the_window`
- `trimmed_audio_keeps_the_media_that_was_playing_at_the_new_start`
- `a_negative_offset_is_silence_until_the_media_origin`
- `negative_offset_draws_silence_before_the_file`
- `a_captured_plugin_state_keeps_redo_and_does_not_add_an_undo_step`
- `a_plugin_hash_keeps_the_fader_preview_without_a_stream`
- `infinite_input_does_not_poison_later_blocks`
- `cached_stop_does_not_start_a_later_region_or_pass_the_loop_end`

画面と機器は開いていない。聴感はユーザー担当である。
