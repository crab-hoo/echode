# 2026-10-27 敵対的監査

## 対象と判定の境界

- 対象: `778c3bfe795b3a13c2802bb28c79e2d2e1bbcae9`。開始時の作業ツリーは変更なし。
- 前回の [現行コード監査](2026-10-25-code-audit.md)（`9f363f9`）以降の 22 コミットと、
  信頼境界（保存・復旧、ヘルパーとの共有メモリと制御チャネル、C ABI、API ソケット）を重点にした。
- 方法は静的照合と、製品と同じ Rust の入口への改ざん・乱数・極端値の入力である。
  乱数の試験は一時ファイルで実行し、リポジトリには残していない（規模は「攻めて問題がなかった範囲」に記す）。
- 実機音声、聴感、実画面、VoiceOver、第三者プラグインは実行していない。
  アプリ・機器・ウィンドウは開かず、既存アプリの終了・置換もしていない。
- 監査時点では製品の処理は修正せず、監査記録と未解決の再現テストだけを追加した。
- Phase 2 以降の未実装機能と、既存の監査・ADR で受け入れ済みの制約は、新規不具合に数え直さない。

## 判定

**モデル・エンジン・ヘルパー境界は、今回の改ざん・極端値の入力に対して堅牢だった。**
**弱点は復旧の信頼境界にあり、壊れたジャーナルの 1 値から、確定と返した編集が通知なく失われる経路を再現した。**

パッケージの JSON とコミットは読込み時に不変条件を検査するが、ジャーナルの変更集合の再生だけが検査しない。
その非対称が、不正なモデルでのオープン、不正な自動保存、再オープン時の黙った後退へ連鎖する（ADV-01、ADV-02）。
監査証拠の保全にも、方針と実態の食い違いがある（ADV-03）。

## 指摘一覧

優先度は [2026-10-13 監査](2026-10-13-audit.md) の定義に合わせる。
P1 は保存の障害処理・制作の正しさ、P2 は保護・運用の不足、P3 は軽微な不整合である。

| ID | 優先度 | 内容 | 状態 |
|---|---|---|---|
| ADV-01 | P1 | 復旧がジャーナルの変更集合を不変条件なしで適用し、確定と返した編集が再オープンで消える | 対応済み・通常回帰へ移行（ADR 0180） |
| ADV-02 | P2 | 復旧が古いコミットへ後退しても、アプリは利用者へ知らせない | 未着手・テストなし |
| ADV-03 | P2 | 監査・ADR の証拠が追跡外の `target/` にあり、大半が既に消えている | 未着手 |
| ADV-04 | P3 | 何も変えない編集が Undo の段とジャーナル行を作る | 未着手・再現テストあり |

### ADV-01: 復旧が不変条件を検査せず、不正な状態を保存・公開する

- 場所:
  - [document.rs](../../crates/echode-history/src/document.rs) の `replay_suffix`（1454〜1473 行）が各行を `apply_changeset` するだけで、結果を検査しない。
  - [changeset.rs](../../crates/echode-model/src/changeset.rs) の `Draft::finish`（1831 行〜）は ID と所有の整合だけを見て、`check_invariants` を通さない。
    モジュールの冒頭は「スマートクリップの規則を走らせない」とし、変更集合を書かれたとおりに適用する設計である。
  - 一方、`read_package` とコミットの読込みは `assemble` から `Project::try_assemble` を通り、`check_invariants`（[model.rs](../../crates/echode-model/src/model.rs) 1323 行）で拒否する。
  - [host.rs](../../crates/echode-history/src/host.rs) の `DocumentHost::open`（164〜183 行）は復旧結果をそのまま公開し、先端がなければ同期的にコミットする。
- 再現 1（先端なし）: 1 秒の音声クリップを拍 0 と拍 8 に置いたパッケージへ、拍 4 への `clip.move` の行を書く。
  その `after` の tick だけを 705,600（拍 1、最初のクリップの内側）へ変えると、`recover` と `DocumentHost::open` が成功する。
  スナップショットの `check_invariants` は「clip … starts before the previous clip on a ends」を返す。
  開いた時点でこの不正な状態が最初のコミットになり、閉じて開き直すと「no readable commit」で開けなくなった。
- 再現 2（先端あり、通常の文書）: 一度自動保存した文書へ、同じ改変の行を連番 `先端 + 1` で書く。
  開くと成功し、続けて `track.add`（名前 `confirmed`）が `Ok` を返す。閉じて開き直すとトラックは `["a"]` だけになった。
  不正なモデルが自動保存で先端になり、読めないコミットとして飛ばされて、親のスナップショットへ戻るためである（ADR 0033 の後退）。
  ジャーナルは先端まで短くされているので、`confirmed` は戻らない。
- debug ビルドでは、再現 2 の最初の編集で `Action::apply` の `debug_assert_invariants`（[action.rs](../../crates/echode-actions/src/action.rs) 942 行）が文書スレッドを panic させ、
  以後の要求は「document thread stopped」になる。release ビルドでは上の黙った消失になる。
- 乱数改変での範囲: 正当なジャーナル行の数値・配列要素・キーを壊して 4,000 回復旧した。
  クリップの重なり（2 種）、ミキサーの値の範囲外、ミキサーの 4 パラメーターの欠落の 4 種類を、不変条件違反のまま受理した。
  同じ改変をパッケージの JSON へ 40,000 回入れた場合、`read_package` は一度も不正な状態を受理しなかった。
- 影響: 起点は完結した行の中の 1 値の破損である。ディスクの破損、同期フォルダ（HIS-7 は未着手）、手作業の編集、第三者から受け取ったパッケージで起こり得る。
  通常のクラッシュの途切れ（AUD-02 で対処済み）とは異なるので P1 とするが、結果は確定と返した編集の消失である。
  行の改ざん検出（チェックサム）はなく、利用者は開いた時点で異常に気づけない。
- 修正条件:
  - 復旧の結果を `check_invariants` で検査し、不成立なら不正な行以降を捨てるか、開くこと自体を拒否する。どちらにするかを ADR で決める。
  - 不正な状態を自動保存のコミットにしない。開いた後の編集が、確定後に消えないこと。
  - debug / release の両方で、下記の 2 件の再現テストが通ること。

#### ADV-01 の対応（2026-10-27）

[ADR 0180](../decisions/0180-journal-replay-invariants.md) で、破損行の切捨てではなくオープン拒否を選んだ。
各行の適用後に不変条件を検査し、違反の連番と理由を返す。公開・初回コミット・自動保存へ不正な状態を渡さない。
上の 2 件は修正前に debug / release の期待したアサーションで失敗することを再確認し、`#[ignore]` を外した。
原本・先端の保持とロック解放、明示修復後の編集の再オープンを追加した。
通常回帰は edit / undo / redo の途中の違反と、ミキサーの範囲外・パラメーター欠落も拒否する。
必須契約 `journal-invariants`・`journal-confirmed-edits` は、不変条件検査を外した負の対照を持つ。
当時の失敗の証拠は下記に保持し、現行の検査結果は検査レポートで確認する。
既に壊れた先端からの後退とその通知（ADV-02）は未対応である。

### ADV-02: 古いコミットへの後退を利用者へ知らせない

- 場所: [document.rs](../../crates/echode-history/src/document.rs) の `first_readable_commit`（1475〜1498 行）は、読めない先端を飛ばして親を使う。
  `DocumentHost::recovered_label`（[host.rs](../../crates/echode-history/src/host.rs) 280 行）は基点のコミットを返すが、
  `crates/echode-ffi` と Swift のどこからも呼ばれていない（`recovered` の検索で該当なし）。
- [ADR 0033](../decisions/0033-journal-log.md) は「先端のオブジェクトだけが壊れたときは親のスナップショットまで戻り、間で消した行は戻らない」を受け入れている。
  ただし、その後退を利用者へ伝える要件はどこにもない。
- 再現: 2 回のオープンでトラック A、B を順に追加し、先端のコミットのオブジェクトを壊す。
  開き直すとエラーも警告もなく `["A"]` になった。`recovered_label` は 1 つ前のコミットを返していた。
- 影響: 利用者は後退に気づかないまま編集を続け、次の自動保存で古い状態が新しい先端になる。ADV-01 の消失も同じ経路で黙って起こる。
- 修正条件: 先端と基点が一致しない復旧を検出してアプリへ伝え、編集を受ける前に表示する。
  機器なしの FFI / Swift の回帰で、通知と、通知なしに編集を受けないことを確認する。

### ADV-03: 監査の証拠が消える場所にある

- 事実: `docs/reviews/` の `../../target/` へのリンク 35 件中 34 件の先が存在しない。
  `docs/` の監査・ADR・スパイク・現在の制約に書かれた `target/…` の参照は、112 件中 77 件が存在しない。
  2 日前の [現行コード監査](2026-10-25-code-audit.md) が挙げた `target/code-audit-20261025/` も既にない。
- [資料の入口](../README.md#監査の読み順) は「監査は発見時点の証拠を残す」とするが、証拠の実体は `.gitignore` された作業用のビルド出力にあり、掃除で消える。
  `check-verification` は文書の整合を検査しているが、これらのリンク切れを止めていない。
- 影響: 過去の失敗・未実施の判定を、後から同じ証拠で検証できない。文書だけが残り、根拠をたどれない。
- 修正条件: 監査が引用する証拠は、追跡される場所へ要約して残すか、再生成の手順として書く。
  文書から `target/` への新しいリンクを検査で検出し、既存のリンクは「当時のローカル記録」と明示する。

### ADV-04: 何も変えない編集が Undo の段を作る

- 場所: [session.rs](../../crates/echode-actions/src/session.rs) の `Session::perform`（64〜75 行）は、成功した操作を差分の有無にかかわらず記録する。
  [host.rs](../../crates/echode-history/src/host.rs) の `Request::Apply`（421〜437 行）も空の差分を公開する。
- 再現: ノートと同じベロシティで `note.velocity` を実行すると `Ok` になり、`"changes":[]` の行がジャーナルに残る。
  最初の Undo は何も戻さず、ノートの追加は残る。
- GUI は単体のベロシティ・幾何の編集で同値を送らないので、影響は主に API / MCP のクライアントである。
  [ADR 0054](../decisions/0054-history-restore.md) の復帰は「差分が空なら Undo もジャーナルも自動保存も増やさない」としており、通常の編集と扱いが揃っていない。
- 修正条件: 差分が空の編集は Undo・ジャーナル・自動保存を増やさない。拒否するか無変更の成功として返すかを決め、MCP の結果にも反映する。

## 攻めて問題がなかった範囲

新規の指摘ではないが、同じ入力で再び探す必要がないように記録する。件数は今回の試験の予算であり、網羅の証明ではない。

- **モデル**: 既存の性質テスト（`properties.rs`）のケース数だけを 4,000 に上げて release で実行し、すべて通った。
  `i64` の端の tick・巨大な長さ・テンポ点・拍子・フェード・素材のフレーム数を、25,000 系列×最大 40 手順で debug ビルド（桁あふれ検査あり）へ入れた。
  panic、拒否後の変化、不変条件違反、変更集合と逆変更集合の往復の不一致はなかった。
- **保存形式**: パッケージの JSON の値・要素・キーを 40,000 回改変し、`read_package` は panic せず、不正な状態を受理しなかった。
  ジャーナル行 40,000 件の改変でも `decode_line` は panic しなかった。
- **再生**: 上の極端値のプロジェクト 1,500 個から、各ノート・クリップの位置と `i64` の端で再生器を 3,326 回作り、
  可変ブロック長とループ・位置移動を描画した。panic と非有限の出力はなく、範囲外は `Time` エラーで返った。CLAP と機器は含まない。
- **ヘルパーとの共有メモリ**: ヘルパーが書ける `frames` とイベント数は、ホスト自身のレイアウトで上限を取る
  （[region.rs](../../crates/echode-sandbox/src/region.rs) の `prepare_block`・`collect_ready`）。
  制御チャネルは 64 MiB の上限と長さ検査を持ち（[control.rs](../../crates/echode-sandbox/src/control.rs)）、パラメーター一覧の確保は本体の長さで上限を取る（[params.rs](../../crates/echode-sandbox/src/params.rs)）。
  非有限のサンプルは加算前に 0 へ置き換える（[mix.rs](../../crates/echode-engine/src/mix.rs) 122〜126 行）。
- **API ソケット**: 接続ごとに `getpeereid` で UID を照合するので、`bind` から権限変更までの隙間は他ユーザーの接続にならない。
  クライアントが接続を中断しても macOS の `accept` は失敗しないことを確かめた。
- **新しい C ABI**: `echode_sample_overview` は null・長さ・UTF-8 を検査する。Swift の初期容量 64 KiB は 512 組のピークに足り、通常は 1 回の復号で済む。
- **ループ（ADR 0156）**: 制御側の `set_loop` が計画を作り直し、音声側は計画の値が変わった時だけ再び有効にする。
  音声側の `SetLoop` 分岐は制御側が先に受けるため通常は通らない。
- **ノートの一括編集（ADR 0176〜0179）**: Swift の計算は桁あふれ検査付きで、送信は 1 つのトランザクションである。
  ホストは途中の失敗で全体を戻す（`Session::transaction`、`apply_transaction`）。空の配列は Undo を積まない。
- **RTSan**: 試聴の IOProc の `fill` も `under_rtsan` の契約テストに含まれる。

## 残るリスク（新規の不具合には数えない）

- **実機の計測**: `spike-audio`・`spike-engine-smoke`・`spike-app`・`spike-draw`・`spike-playhead` は、最新の検査記録でも理由付きで見送られ、
  `target/verification/` に現在のログがない。その間に音声経路の ADR 0133・0136・0142・0147・0155・0156・0160 が入った。
  ユーザー担当とする扱いは方針どおりだが、最後に実機で通った版を追跡できない（ADV-03 と同じ原因）。
- **負の時刻の自動窓**: [ADR 0170](../decisions/0170-content-relative-auto-bars.md) はモデルで許すと決めている。
  リンクした配置からの編集で時刻 0 より前へ出たノートは、Swift の `NoteGeometry.isValid`（開始 ≥ 0）により、元の配置側では選択・移動の対象外になる。
  コードからの推定で、実画面では確認していない。
- **`make known-bugs`**: 監査前は無視された `audit_` テストが 0 件で、何も実行せずに成功していた。今回の 3 件で再び意味を持つ。
- 2026-10-08 監査の未着手項目（UI-4 など）は元の一覧が正本であり、この監査では状態を変えない。

## 再現テストと証拠

期待する正常動作を assert し、`#[ignore]` で登録した。通常の自動検査へ予定された失敗を混ぜず、`make known-bugs` の `audit_` フィルターで実行する。

- [audit_recovery.rs](../../crates/echode-history/tests/audit_recovery.rs)
  - `audit_adv01_recovery_refuses_a_journal_line_that_breaks_invariants`（ADV-01 の再現 1）
  - `audit_adv01_an_edit_confirmed_after_recovery_survives_reopening`（ADV-01 の再現 2）
  - `audit_adv04_an_edit_that_changes_nothing_adds_no_undo_step`（ADV-04）

```sh
cargo test -q -p echode-history --test audit_recovery -- --ignored
cargo test -q --release -p echode-history --test audit_recovery -- --ignored
```

どちらも終了コード 101 で、3 件すべてが期待値の不成立で失敗した。
ADV-03 のとおり `target/` のログは残らないので、主な出力を次に要約する。

- 再現 1: `recover returned a project that breaks the model rules`、左辺は `clip … starts before the previous clip on a ends`。
- 再現 2: release は `an edit reported as saved was lost on reopen: ["a"]`。
  debug は文書スレッドが `invariants.rs:269` で panic し、編集が `document thread stopped` を返した。
- ADV-04: 最初の Undo の後もノートが 1 件残る（期待 0）。

ADV-01 の「先端なしで再オープン不能」と ADV-02 の再現は、監査用の一時テストで確認し、登録していない。
手順は各指摘の本文に記した。

## 保守と未確認

- [資料の入口](../README.md#監査の読み順)、[現在の制約](../status.md)、[デバッグ手順](../debugging.md) にこの監査と症状を追加した。
- テストは既存の `echode-history` の入力範囲と `known-bugs` の入口に載るため、新しい検査 ID・Makefile ターゲットは作らない。
- 保存・音声・ABI の実装と依存は変えていない。新しい診断データ形式や収集先も不要である。
- 実機音声、聴感、第三者プラグイン、実画面・VoiceOver は実行していない。
