# 2026-10-30 構造の横断監査

## 対象と判定の境界

- 対象: `bf73678670c93a8aea564337a2106ac9422407f1`。開始時の作業ツリーは変更なし。
- 目的は、今後の機能追加で絡み合い（スパゲティ化）を起こしやすい構造を、根本の修正方針と一緒に挙げること。
  個々の不具合探しではなく、同じ種類の修正を繰り返させる形を対象にした。
- Rust 14 crate・Swift（EchodeApp / EchodeBridge）・検査基盤を、規模、変更頻度（2026-10 のコミット）、
  層をまたぐ重複、状態の持ち方、呼出しの通り道で照合した。全行の通読ではない。
- SPG-01 だけは構造の欠陥が実際の誤動作になることを、製品のヘルパーとテスト用プラグインで再現した。
  ほかの項目はコードの読解と計数に基づく指摘であり、誤動作を実証したものではない。
- 監査時点では製品の処理は修正せず、監査記録と未解決の再現テストだけを追加した。

## 指摘一覧

優先度は、P1 が再現済みの誤動作を伴う構造、P2 が同種の不具合の再発が既に記録されている、
または次の機能追加で確実に手数が増える構造、P3 が整理すれば保守が楽になる構造を表す。

| ID | 優先度 | 内容 | 状態 |
|---|---|---|---|
| SPG-01 | P1 | ヘルパーのメインスレッド要求が 1 つの共有バイト列と種類番号を使い回し、応答に要求の識別がない | 対応済み（後述） |
| SPG-02 | P2 | 文書ごとの状態が `ProjectController` に散在し、文書の照合が手書きの 63 か所に分かれる | 未着手 |
| SPG-03 | P2 | Swift から Rust への編集要求だけが型のない辞書で、30 種類の操作の形を二重に持つ | 未着手 |
| SPG-04 | P2 | 時間単位・値の範囲を Swift が数値の直書きで重複して持つ | 定数・変換に対応、下書きの整数化は未着手 |
| SPG-05 | P2 | 文書の処理の調停（保存の取込み・デバイス要求の待ち）が FFI の crate に入っている | 未着手 |
| SPG-06 | P3 | プラグイン操作 1 つが約 9 層の素通しを通り、一部の素通しはテストだけが使う | 旧素通し 3 件を削除、残りは未着手 |
| SPG-07 | P3 | 再生の生成関数が 6 種類あり、本体の重複と引数の増加が続く | 未着手 |
| SPG-08 | P3 | ピアノロールの排他的な操作状態を 9 個の独立した Optional で持つ | 未着手 |
| SPG-09 | P3 | モデルの項目を 1 つ足すと変更セットの 6 か所以上と 30 前後のファイルに手が入る | 未着手 |
| SPG-10 | P3 | Phase 0 のスパイクの別モデルが製品 crate に同居する | 未着手 |
| SPG-11 | P3 | 製品コードの 141 関数がエラーを文字列で返し、原因の区別を失う | 未着手 |

### SPG-01: ヘルパーのメインスレッド要求が共有バイト列を使い回す

- 場所: [threads.rs](../../crates/echode-plugin-host/src/threads.rs) の `Bridge`（165 行〜）、
  種類番号 `STATE_SAVE`〜`PARAM_PARSE`（60〜68 行）、`service_state`（359 行〜）、
  `call_plugin_operation_within`（1927 行〜）、`perform_plugin_state`（1979 行〜）。
  親側は [session.rs](../../crates/echode-sandbox/src/session.rs) の `wait_message`（558 行〜）。
- 状態の保存・読込み、イベント容量、パラメーターの一覧・読取り・中継・解釈、GUI の表示・終了の 9 種類を、
  `u8` の種類番号・共有の `state_bytes`・`state_error`・`param_read`・世代の組で 1 本の経路へ多重化している。
  入力と出力が同じ `state_bytes` で、要求ごとの領域がない。
  機能を足すたびに種類番号と `match` の腕、バイト列の手書きの符号化（容量の LE、題名の UTF-8、報告の独自形式）が増えた。
- 既定の腕 `_ => load_plugin_state`（2017 行）は、未知の種類番号を状態の読込みとして実行する。
  種類を足して腕を書き忘れると、入力バイト列がプラグインへ状態として読み込まれる。
- `Bridge` には約 50 個のアトミックがあり、要求と完了の世代の組（再起動・状態・パラメーター・コールバック・メインスレッド）を
  それぞれ手書きの規約で扱う。どの組がどのスレッドの所有かは型に現れない。
- 親側は応答を種類で照合し、要求の識別子を持たない。時間切れになった要求の遅れた応答は、次の同種の要求の応答として消費される。
- 再現: `slow-save` の故障で初回の状態保存を 3 秒止め、保存（2 秒で時間切れ）の直後に `[128]` を復元する。
  ヘルパーは遅れて終わった保存の出力 `[64]` を復元の入力として読み込み、`Dirty` 通知を出した。
  復元の応答には前の保存の `plugin state timed out` が返った。
  GUI の表示（8 秒待ち）が時間切れになった後の操作では、GUI の報告のバイト列が状態やパラメーター ID として読まれ得る。
- 根本の修正方針:
  - メインスレッドへの要求を `enum MainRequest`、結果を `enum MainReply` にし、要求ごとに連番と入出力を 1 つの領域へまとめる。
    完了は連番で照合し、前の要求が終わるまで次を出さないか、明示的に「処理中」で拒否する。種類番号と既定の腕をなくす。
  - 制御ソケットの要求と応答に要求番号を付け、親は番号の違う遅れた応答を捨てる。
  - `Bridge` のアトミックを、領域の交換・状態・パラメーター・GUI・再起動の小さな構造体へ分け、所有スレッドを型と注釈で示す。
- 修正条件: `audit_spg01_a_timed_out_save_does_not_replace_the_next_restore_bytes` の `#[ignore]` を外して通ること。
  既存の隔離回帰（`hosted_playback`・`isolation`・`sine_pad`）と RTSan が通ること。

#### 後続対応（2026-10-30）

[ADR 0199](../decisions/0199-owned-helper-control-requests.md) により、型付きの専有要求と結果・処理中の明示拒否を導入した。
状態要求の共有バイト列・種類番号・別枠のパラメーター結果を除き、制御応答は要求番号で照合する。
上の再現テストは `#[ignore]` を外し、完了後の復元・保存と出力サンプルの検査を追加した。
`main_requests_tests.rs` と `session_correlation_tests.rs` も、古い完了・応答の非干渉を検査する。
時間切れは結果不明であり、プラグインを強制的に取消すものではない。
残りの `Bridge` のアトミック全体の分割は行っていない。

### SPG-02: 文書ごとの状態と文書の照合が散在する

- 場所: [ProjectController.swift](../../swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift)。
  2026-10 のコミットの 38 件が変更した最多のファイルで、17 ファイルが `extension ProjectController` で状態と処理を足している。
- 保存される約 60 個のプロパティに、アプリの寿命（出力・プラグイン名）、文書の寿命（選択・ピアノ・ミキサーの仮表示・
  パラメーター一覧・プラグインの窓）、操作の寿命（ジェスチャ・待ち行列の末尾の Task）が混在する。
- `open` と `close` が、それぞれ約 25 個の項目を手で初期化する（172〜232 行、509〜546 行）。
  `forgetParameters` と `stopPluginActivity` が残りを受け持つ。新しい文書の状態を足すたびに 2〜4 か所の更新が要る。
- 文書の照合 `documentID ==` は 30 ファイルに 63 か所あり、各機能が別々に書いている。
  編集の入口は `documentID: UUID? = nil` を受け、`nil` は「今の文書」として照合を省く（703 行）。
- 同じ種類の不具合が [ADR 0193](../decisions/0193-header-mix-gesture-document-scope.md) と
  [ADR 0194](../decisions/0194-clip-mix-gesture-document-scope.md) で同日に 2 回直された。
  古い押下が文書交換後に新しい文書のコールバックを借りる型であり、操作子ごとの照合では再発を防げない。
- 根本の修正方針:
  - 文書の寿命の状態を `DocumentSession`（参照型）へまとめ、`open` / `close` はこれを丸ごと差し替える。初期化の列挙をなくす。
  - 文書に対する操作は省略できない `DocumentScope`（文書 ID と弱参照の組）を受け、押下時に捕まえる。
    `scope.edit { }` が照合を 1 か所で行い、`documentID: UUID? = nil` の既定値を廃止する。
  - 文書をまたがないアプリの状態（出力、プラグイン名、試聴）は別の型へ分ける。

### SPG-03: 編集要求だけが型のない辞書である

- Rust は [action.rs](../../crates/echode-actions/src/action.rs) の `define_actions!` が 30 種類の操作の唯一の一覧で、
  JSON Schema も生成している。
- Swift は 11 ファイルで `["type": "note.edit", ...]` のような `[String: Any]` を組み、
  `JSONSerialization` で `Document.perform` へ渡す（`[String: Any]` は 15 か所）。
  音声・デバイス・履歴の要求は `Encodable` の型であり、最も種類の多い編集要求だけが型を持たない。
- キー名や型の誤りは Rust の `deny_unknown_fields` で実行時に拒否されるが、Swift のコンパイル時には分からない。
  Swift のテストが通る経路以外は検査されない。Rust 側の項目の改名は Swift の文字列を黙って壊す。
- 根本の修正方針: `catalog()` と各操作のスキーマから Swift の `enum EditAction: Encodable` を生成するツールを置き、
  生成物をコミットする。`make check` で生成物が最新であることを検査し、`Document.perform` は `[EditAction]` を受ける。

### SPG-04: 時間単位と値の範囲を Swift が直書きする

- Swift の製品コードに `705_600`（1 拍の tick 数）が 19 ファイル・77 か所ある。
  モデルの正本は [time.rs](../../crates/echode-model/src/time.rs) の `TICKS_PER_QUARTER` である。
- ミキサーの範囲 `0...4`・`-1...1` とベロシティの `127` も Swift に直書きされ、
  正本の `GAIN_MAX` などと結び付いていない。
- 拍と tick の変換 `PianoCamera.ticks` を、ピアノロールと無関係なトランスポート（`locate`、ループ範囲）も使う。
  ループ範囲は `Double` の拍で持ち、送る直前に tick へ丸める。
- 根本の修正方針: 定数を cbindgen のヘッダへ公開し、Swift の `Tick` 値型と `ModelLimits` の 1 か所から使う。
  変換はその型に置き、UI の座標計算だけが `Double` を使う。機械的な置換で済み、SPG-03 の前に行うと生成コードも同じ型を使える。

#### 後続対応（2026-10-30）

[ADR 0200](../decisions/0200-shared-model-units-and-limits.md) でモデルの単位・範囲を値で返す C ABI と、
Swift の `ModelLimits`・`Tick` を導入した。
アレンジ・ピアノロール・トランスポートの定数と変換を共有し、ピアノ専用の変換への依存を除いた。
既存の丸め・範囲・保存形式は保ち、製品のノート・再生位置・ミックスと再オープンを通常回帰で照合する。
ループの UI 下書きの整数化と、既存の全時刻フィールドを値型で包む部分は行っていない。

### SPG-05: 文書の処理の調停が FFI の crate にある

- 場所: [queue.rs](../../crates/echode-ffi/src/queue.rs) の `worker`（554 行〜、約 240 行）。
- C ABI の変換を担う crate が、履歴（`DocumentHost`）・音声（`Session`）・デバイスの通り道・プラグインの保存の取込み
  （`pending_saves`・`editing`）・出力待ちのデバイス要求の調停を持っている。
  CLI と MCP は同じ調停を再利用できず、ヘッドレスの経路と製品の経路の差になり得る。
- `catch_unwind` と受付停止・完了通知の分岐が 3 回、文書が変わった時の保存と編集中の破棄が 2 回、ほぼ同じ形で書かれている。
- 根本の修正方針: 調停を FFI の外（新しい文書サービスの crate、または `echode-audio-io`）へ移し、
  `DocumentService::handle(Operation) -> Outcome` と 1 つの保護付き実行の補助にまとめる。FFI は C ABI と完了通知だけを持つ。

### SPG-06: プラグイン操作の素通しの層

- パラメーターの一覧の 1 回は、Swift → FFI の `device.rs` → `PluginControl` → `ControlState` → ソケット →
  ヘルパーの制御スレッド → 種類番号 → メインスレッドへ進む。
  エンジン経由の旧経路として [playback.rs](../../crates/echode-engine/src/playback.rs) の
  `device_parameters`・`set_device_parameters`・`capture_device_state`（1841〜1868 行）が残る。
- この 3 つの製品からの呼出しは 0 件で、テストだけが使う。テストが製品と違う経路を通る。
- `PluginHost`・`PluginControl`・`ControlState`・`PluginAudio` が同じ操作の同名メソッドを重ねて持ち、
  操作を足すたびに 3〜4 個の素通しが増える。
- 根本の修正方針: テストを製品と同じ `device_control_handle` の経路へ寄せ、使われない素通しを消す。
  SPG-01 の型付き要求と合わせ、`PluginControl` は 1 つの `request(PluginRequest)` を持つ形へ寄せる。

後続対応（2026-10-30）: テストを製品と同じ `device_control_handle` へ移し、上記の旧素通し 3 件を削除した。
`PluginControl` の公開操作を汎用の 1 要求へまとめる部分は未着手である（[ADR 0199](../decisions/0199-owned-helper-control-requests.md)）。

### SPG-07: 再生の生成関数の増殖

- `playback_pair`・`playback_pair_at`・`playback_with_plugins`・`playback_with_plugins_at`・
  `playback_with_plugins_in_workgroup_at`・`playback_with_clock` の 6 種類があり、呼出しは 153 か所（多くはテスト）。
- 後ろの 2 つはワークグループの有無以外は同じ本体で、`too_many_arguments` を許している。
  開始位置・プラグイン・ワークグループ・時計の組合せが増えるたびに関数が増える。
- 根本の修正方針: `PlaybackOptions { start, clock, plugins: Option<PluginSetup> }` の 1 つの入口にし、既存の関数は薄い互換か削除にする。

### SPG-08: ピアノロールの操作状態

- 場所: [PianoRollView.swift](../../swift/EchodeApp/Sources/EchodeAppCore/PianoRollView.swift) の `PianoGridView`。
- 同時に 1 つしか起きない操作を、`gesture`/`preview`、`velocityGesture`/`velocityPreview`、`moveGesture`/`movePreview`、
  `resizeGesture`/`resizePreview`、`insertion`、`selectionPress`、`rulerPress` の独立した Optional で持つ。
- `mouseDown`・`mouseDragged`・`mouseUp` がそれぞれ全種類を順に調べ、`cancelGesture` が全部を消す。
  ADR 0167〜0186 の操作追加ごとに、3 つの入口と取消の全部へ分岐が増えた。
  ありえない組合せ（2 種類の操作が同時に進行中）が型で禁止されていない。
- 根本の修正方針: `enum PianoInteraction { case idle, ruler(...), velocity(...), move(...), resize(...), single(...), insert(...), marquee(...) }`
  の状態機械にし、各入口は現在の状態の `switch` だけにする。

### SPG-09: モデルの項目追加の手数

- 場所: [changeset.rs](../../crates/echode-model/src/changeset.rs)（2210 行）。
- 項目を 1 つ足すと、`Property`・`Value`、`diff_*_fields`、`*_change`、`Draft` の状態構造体、
  `Draft::from_project`・`insert`・`set`（約 290 行の `match`）・`finish`、`field_*` の補助を手で揃える。
- デバイスの `offline` は変更セットだけで 15 か所、製品コード全体で約 30 ファイルに現れる。
  エンジン・UI の扱いは必要な変更だが、モデル・保存・FFI のスナップショットの通り道は機械的な重複である。
- 根本の修正方針: `define_actions!` と同様に、オブジェクトごとの項目表（マクロ、または `fields()` / `set()` を持つトレイト）から
  差分・作成・設定・逆変換を生成する。影響が大きいので、`properties.rs` の性質テストを先に厚くしてから行う。

### SPG-10: スパイクの別モデルが製品 crate に同居する

- `echode-history` の `project.rs`・`format.rs`・`store.rs` は Phase 0 の性能測定用で、
  製品の `echode_model::Project` とは別の `Project`・`Track`・`Note`、別の `TICKS_PER_BEAT`、別の Git 保存 `History` を持つ。
  `echode-engine` の `spike` も同様である。
- 名前が製品の型と衝突し、エージェントや補完が誤った型を選び得る。
- 根本の修正方針: `spikes/` 配下の crate か `spike` の feature へ移し、製品のビルドから外す。

### SPG-11: 文字列のエラー

- 製品コード（テスト以外）の 141 関数が `Result<_, String>` を返す。多いのは `echode-audio-io` の
  `stream.rs`（23）・`opening.rs`（12）・`session.rs`（11）。
- 文字列の内容で分岐する製品コードは見つからなかったが、時間切れ・拒否・停止の区別が型に残らない。
  SPG-01 の時間切れを呼出し側が正しく扱えない一因でもある。
- 根本の修正方針: crate の境界ごとにエラーの列挙型を置き、文字列化は FFI と表示の直前だけにする。優先度は低い。

## 保たれている構造

- モデルの操作は `define_actions!` が唯一の一覧で、ジャーナル・API の型・カタログ・スキーマを生成している。
- C ヘッダは cbindgen の生成物で、音声・デバイス・履歴の要求は Swift でも型を持つ。
- 検査の選択は `scripts/verification.json` に集まり、契約・否定対照・期限の仕組みがある。

## 修正の進め方

依存の少ない順に、1 項目ずつ回帰を保って進める。

1. SPG-01: 再現済みの誤動作であり、範囲はヘルパーと親の制御の通り道に閉じる。SPG-06 の素通しの整理を同時に行う。
2. SPG-04: 機械的な置換で、SPG-03 の生成コードの土台になる。
3. SPG-03: 生成ツールと `make check` の最新性検査を足し、Swift の辞書を置き換える。
4. SPG-02: `DocumentSession` と `DocumentScope` を入れる。SPG-03 の型付き要求で編集の入口が少なくなってから行う。
5. SPG-05、SPG-07、SPG-08。
6. SPG-09 は性質テストを厚くしてから。SPG-10、SPG-11 は他の作業のついでに進める。

## 再現テストと証拠

- [audit.rs](../../crates/echode-plugin-host/tests/audit.rs) の
  `audit_spg01_a_timed_out_save_does_not_replace_the_next_restore_bytes`（監査時点は `#[ignore]`、`make known-bugs` で実行）。
  テスト用プラグインに初回の状態保存だけを 3 秒止める `slow-save` の故障を追加した。
- 監査時点の結果（終了コード 101）:
  `the helper loaded bytes other than the requested [128]: level 0.25, restore Err(Protocol("plugin state timed out"))`。
- 規模・変更頻度・重複の数は、監査時点のコミットに対する `wc`・`git log --since=2026-10-01`・`grep` の計数である。

## 保守と未確認

- 実機・実プラグインでの時間切れの発生頻度は測っていない。重い状態や初回の GUI 表示で 2 秒・8 秒を超えるプラグインでは起こり得る。
- SPG-02〜11 は監査時点の再現テストを持たない。修正の作業で、各項目の回帰の形を決める。
  SPG-04 の後続の構造整理には通常回帰を追加した。
