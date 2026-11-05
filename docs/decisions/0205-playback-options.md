# 0205. 再生の生成を 1 つの入口と `PlaybackOptions` へまとめる

- 日付: 2026-10-31
- 状態: 採用
- 対象: [構造監査](../reviews/2026-10-30-structure-audit.md) SPG-07
- 関連: [ADR 0113](0113-product-macos-workgroup-and-caches.md)、[ADR 0147](0147-output-prewarm-and-background-media.md)

## 背景

開始時の HEAD は `46866f2`。
再生の生成関数が `playback_pair`・`playback_pair_at`・`playback_with_plugins`・`playback_with_plugins_at`・
`playback_with_plugins_in_workgroup_at`・`playback_with_clock` の 6 種類あった。
ワークグループの有無だけが違う 2 関数は本体を複製し、`too_many_arguments` を許していた。
開始位置・プラグイン・ワークグループ・時計の組合せごとに関数が要り、時計とプラグインの組合せは書けなかった。
負の開始位置の拒否も `playback_pair_at` の経路だけにあった。
これらは読解に基づく指摘で、誤動作は実証していない。

## 決定

- **1 つの入口**: `playback(project, rate, frames, PlaybackOptions)` だけを公開し、旧 6 関数は互換を残さず削除する。
  製品 3 か所とテストの呼出しを機械的に移した。テストが製品と別の入口を通らない。
- **`PlaybackOptions`**: `default()` は内部時計の 0 サンプルでヘルパーなしとする。
  `at`・`clock`・`plugins`・`workgroup` は互いに組み合わせられる。新しい選択はこの型へ足し、関数を増やさない。
- **時計は 1 つ**: 監査の案の `start` と `clock` の 2 欄は矛盾を表現できるので、`at` は内部時計への差替えとする。
  `at` と `clock` は後に呼んだほうが有効である。
- **負の開始位置**: 時計の種類によらず `playback` の入口で `PlaybackError::Time` を返す。ヘルパーの起動前に拒否する。
- **ワークグループ**: `plugins` と独立させ、トランスポートの寿命の間保持する。
  製品の `replace` はヘルパーの実行ファイルを探し直すので、開く時にヘルパーがなくても後から起動するヘルパーが参加する。
- **テスト内の補助**: `hosted_playback.rs` と `sine_pad.rs` の同名の補助は Sine Pad の経路を足す固定入力であり、残す。

## 変わる挙動

- ヘルパーの実行ファイルがなくワークグループがある時、旧経路は空のパスで `HostGraph::open` を呼び、
  チェインごとの段なしのトラックを作っていた。新しい経路は、ワークグループなしの時と同じく空のグラフのまま開く。
  段がないのでどちらも処理するプラグインはなく、ワークグループの保持も同じである。出力の音は変わらない。
- 独自の時計の負の位置を受け付けなくなった。製品の時計は内部時計だけで、独自の時計はテストの 1 か所が 0 で使う。

## 回帰と基盤の保守

- `every_option_rejects_a_negative_start_before_opening_helpers` は全経路での負の開始位置の拒否を確認する。
- `start_clock_and_plugins_combine_and_the_last_start_choice_wins` は旧 API で書けなかった組合せと、後勝ちを確認する。
- `a_workgroup_given_without_helpers_reaches_helpers_started_later` は実ヘルパーの経路で、後から起動する時のワークグループの使用を確認する。
- 必須契約 `live-workgroup` の反例を、新しい代入 `control.workgroup = workgroup;` へ更新した。
  `startup-position` の反例の文字列は `at` の本体に残るので、変更不要である。
- 音を作る処理と RT の経路は変わらないので、ブロック長回帰の追加は不要である。
  ログ形式・出力先も変わらないので、`doctor` と `debug-report` の収集範囲とデバッグ手順の更新は不要である。
