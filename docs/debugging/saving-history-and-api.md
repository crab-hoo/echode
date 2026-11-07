# デバッグ手順: 保存・履歴・API / MCP

保存・パッケージの読込み・ジャーナルと復旧・Undo / Redo・JSON-RPC と MCP の症状を切り分ける。
最初の診断と記録する項目は [デバッグ手順](../debugging.md) にある。症状の一覧は `--outline` で見る。

## JSON-RPC・API

### JSON-RPC が `-32600` を返す・ID なしの入力にもエラーを返す

`rpc.rs::ApiSession::reply` の外枠検証を確認する。
`jsonrpc` と `method`、ID の型、明示した params がオブジェクト・配列かを照合する。
明示した `params:null` は省略と異なり不正で、メソッドの引数エラー `-32602` と区別する。
正常な通知だけ応答を省略し、壊れた外枠は ID がなくてもエラーにする。
`request_validation.rs` の公開 API と実ソケットの回帰で、拒否時のジャーナルと Undo / Redo 保持、
後続要求の応答を確認する（[ADR 0020](../decisions/0020-json-rpc.md#要求の外枠の検証2026-10-29)）。

### JSON-RPC の前後に空白がある時だけ `-32700` になる

JSON の空白は SP・HT・CR・LF に限られる。垂直タブ・改ページ・ノーブレークスペース等は、見た目が空白でも不正である。
`rpc.rs::ApiSession::reply` が Unicode 全体の `trim()` で不正文字を消していないか確認する。
文字列の外側の構文エラーと、名前など文字列内の正当な Unicode を区別する。
`request_whitespace.rs` と必須契約 `rpc-json-whitespace` は公開 API と実ソケットで、拒否後の履歴と後続要求を検査する。
入力を自動修正して再送せず、元の入力と応答を照合する（[ADR 0020](../decisions/0020-json-rpc.md#json-の空白だけを除去する2026-10-29)）。

### API が `request line ended without a newline` で接続を閉じる

EOF の前に LF が届いていない。JSON 自体が正しくても、この部分行は編集・通知・Undo / Redo へ渡さない。
`ApiSession::reply` の直接呼出しと、`serve_connection` の通信境界を区別する。
完了済みの前方の要求は成立している場合があるので、接続全体を未実行と決めて再送しない。
`request_framing.rs` と `rpc-request-framing`、`serve_tests.rs` の行上限の境界で再現する
（[ADR 0073](../decisions/0073-socket-path.md#改行前の切断の拒否2026-10-29)）。

### API / MCP の同値設定で Undo が何も戻さない・Redo が消える

無変更は成功として Action／配列を返すが、新しい Undo を意味しない（[ADR 0182](../decisions/0182-noop-edits.md)）。
`Session` の空差分判定と束の正味の変更、`DocumentHost` のジャーナルの増加判定を確認する。
`noop_edits.rs` と MCP の `contract_mcp_noop_edits_succeed_without_consuming_history_or_redo` で切り分ける。
画面だけ件数が増える時は、perform 完了の `historyReport.changed` と `recordEdit` の条件を照合する。
旧版の `changes: []` の行は自動削除しない。同値の依頼・前後の連番・Redo の有無を記録し、原本を保持する。

### API / MCP の呼出しが返らず、後続クライアントも止まる

ソケットの読込み待ちだけでなく、応答の書込み待ちを確認する。
`serve_connection_limited` は読書きの無進行を 30 秒で打ち切る。
`could not write API reply` は応答送信の失敗であり、編集そのものの失敗とは限らない。
`audit_cur04_an_unread_reply_releases_the_connection` は送信バッファを満たし、後続接続と編集の単一実行を通常回帰で確認する。
編集が既に記録された可能性があるので、同じ編集を無条件に再送しない（[CUR-04](../reviews/2026-10-25-code-audit.md#cur-04-api-の書込みにはタイムアウトがない)）。

## 保存・パッケージ・復旧

### ミキサーのパラメーター ID を置換した差分が `InvalidChange` になる

`Project::diff` の前後でデバイス ID が同じでも、子のパラメーター ID が変わる場合は、`ParamOrder` の更新だけでは再生できない。
新 ID の `Parameter` の作成と旧 ID の削除が含まれるか、`changeset::device::diff_device_fields` を確認する。
`cargo test -q -p echode-history --test parameter_changesets` は、公開の組立てから一部・全部の置換を再現する。
独立した期待順序、2 ミキサー、値変更との混在、任意の浮動小数点ビットとジャーナル経由の正逆適用で切り分ける。
これはモデルの子の ID であり、CLAP が公開するパラメーター番号やプラグイン GUI の変更とは別である。
既に作成レコードのないログから失われた値を推測せず、未列挙の子・不明な所有者の拒否は維持する。

### クリップが増えると読込み・ノートの編集が急に遅くなる

`make spike-scale` の `model_edit` にある `assemble_ms` と `edit_note_ms` を、64 / 128 トラック × 64 クリップで比べる。
機器・プラグインは使わない。編集の操作感全体ではなく、モデル単体の時間である。
モデルの編集が速くても確定が遅い時は、同じ行の `diff_ms` で変更セット生成を分ける。適用・保存・通知はこの値に含まない。
`model_scale_tests::diff` は同一性のキー訪問を前後の内容数の和に制限し、共有内容のノートを辿らないことも数える。
`changeset_content_diff::ChangedContents` は ID 順の列をマージし、`Arc::ptr_eq` で非共有と判定した内容だけをノート比較へ渡す。
既存内容間でノートを移した時の所有者・変更順と、内容 ID の全交換・部分交換も同じ回帰で調べる。
別組立ての同値内容や、作成・削除を誤って省略する不具合は同じ回帰の往復で切り分ける。
時間に依らない回帰は `cargo test -q -p echode-model model_scale_tests` で、内容のキー・配置の参照・重なりの比較・親の探索を数える。
共有 MIDI 内容の配置数だけで遅くなる時は、同じ回帰の `shared_content_validation_does_not_rescan_notes_per_placement` を確認する。
ノートの検証回数も数え、内容のノート数と配置数の積に増えないことを検査する。
`invariants::tests` の整数上下端と自動小節の回帰で、両端の集計によって範囲外のノートを見逃していないかを切り分ける。
共有内容の追加・編集・複製だけが遅い時は、`shared_content_edits_do_not_rescan_notes_per_placement` の検証回数を確認する。
`placement::check_shared_notes` は変更後の内容を一度集計する。`placement::tests` の独立した i128 の照合と整数上下端の製品編集で、配置ごとの両端検証の範囲を調べる。
共有配置の検証前の所属探索は `note_edits_do_not_search_unrelated_contents_after_refitting` で、関係のない内容のノート数に増えないことを調べる。
再適合・結合の戻り値が残った内容を指すかは `placement::tests::merged_note_edits` で確認し、結合先のリンクの範囲外を見落とす不具合と分ける。
固定リンクから別トラックの自動配置を編集した時だけ遅い場合は、`model_scale_tests::refit::linked_refit` の同期キー訪問とトラック複製回数を調べる。
`placement::refit_auto` は配置 ID の一時索引で付替えを戻し、`retarget` と共に無関係なトラックを検査のために複製しない。
この計数は再適合・付替えの範囲だけであり、ID 重複の探索・参照集合・保存待ちの費用とは分ける。
結合数が多い時だけ遅い場合は、`model_scale_tests::merge::multi_clip_merge_discovery_visits_autos_at_most_twice`
と逆向きの連鎖の回帰で候補の収集・探索回数を調べる。
`refit_current` は開始順・非重複の窓から候補を集め、左右の前線を各一度進める。
`merge::hosts::reverse_chain_discovery` は追加・編集・複製・削除・別トラックのリンクからの経路も検査する。
固定窓が多い時は `model_scale_tests::merge::multi_clip_merge_wall_queries_do_not_rescan_placements` の壁索引の構築・問い合わせの訪問数を確認する。
`placement::FixedBars` は固定窓を一度集めて二分探索する。計数には整列の比較を含めず、編集全体の費用とは区別する。
接する候補のない編集で索引を作らないことは、同じ回帰群の `multi_clip_merge_wall_index_is_lazy_without_touching_autos` で検査する。
負の位置・同じ小節の短い窓・包含する窓・整数上下端の照合は `placement::tests::fixed_bar_index_matches_scalar_overlap_queries` を使う。
候補の前線探索は `placement::host_tests` の反復走査との独立照合で確認する。窓が初期範囲を含む場合・窓外へのノート移動・小節を共有する壁・整数上下端も調べる。
深いフォルダだけで遅くなる時は、`model_scale_tests` の `nested_forest_validation_does_not_revisit_resolved_ancestors` と逆順入力の回帰で親の訪問回数を確認する。
親の存在・循環は `hierarchy::check_forest`、順序キーと配列の並びは `model::arrange_fault` が検証する。
`hierarchy::tests` は小さい親グラフの全列挙と深い逆順の鎖を調べ、順序違反と循環を切り分ける。
親を変える操作だけで遅い時は `model_scale_tests::nested_reparent` で、移動と循環の拒否の親訪問回数を確認する。
`set_parent` は変更後のグラフを同じ `check_forest` で検証し、同じ親ならグラフの検証を行わずに返す。
`content id order` の拒否は二分探索の前提の破れであり、保存入力を整列する組立てと、変更セット復元の内容の順を調べる。
モデルが速くても `replace_p50_ms` が遅ければ計画の使い回し（ADR 0274）、保存で待つなら FCA-14 を別に調べる。
ID 重複の線形探索と、毎回全配置から作る参照集合は残る（[ADR 0242](../decisions/0242-model-content-lookup-and-references.md)）。

### テンポ点が多いと無関係な編集の確定も遅くなる

テンポ点が多い時だけ確定が遅い場合は、`cargo test -q -p echode-model model_scale_tests::tempo_diff` で点の訪問数を確認する。
`changeset::tempo::diff_tempo` は共有点配列を省略し、別配列の時だけ時刻順に一度マージする。
修正前は同じ 64 点の別組立ての比較が 320 回訪問し、共有された単一点でも 5 回訪問していた。
通常回帰は共有を 0 回、別配列を前後の点数の和以内に制限し、独立した時刻マップで変更順と正逆適用を照合する。
整数の上下端・分数テンポとジャーナルの互換は履歴の `tests/tempo_changesets.rs` でも確認する。
この計数は差分生成だけで、テンポ編集時の索引の構築・配置の再適合・保存同期の時間を含まない。

### FX が増えるとチェインやミキサーの編集確定が遅くなる

`cargo test -q -p echode-model model_scale_tests::device_diff` は、製品の `Project::diff` でデバイスキーの検査数を数える。
`changeset::diff_devices` の旧チェイン索引は照合時に一致分を除き、作成・更新を次順、削除を旧順で出す。
長いチェインのバイパス・ミキサー値・並べ替えだけでなく、チェイン内の全 ID の交換と別組立ての同値の版も検査する。
変更順と保存・Undo の不整合は、同じ回帰の正逆往復と履歴の `tests/device_changesets.rs` で切り分ける。
生成が速くてもジャーナルの再生が遅い時は、`model_scale_tests::device_apply` で適用側のデバイス所属の訪問数を分けて確認する。
`changeset::Draft::finish` は所属数を一括集計し、各トラックの子の列と照合する。適用後のデバイス数以内の訪問に制限する。
同じ回帰の不正な変更セットは未列挙・不明な ID・別の所有者・所有者だけの削除を区別し、拒否後の元の版も照合する。
この検査はプラグインを起動せず、ヘルパーの通信・状態取得・再生計画・ジャーナル同期の遅延は含まない。
`spike-scale` の既存の `model_edit` に長い FX チェインはないため、その時間を本症状の再現に代用しない。

### 素材やキャッシュが Git の除外対象にならない

パッケージ直下の `.gitignore` に `/media/objects/` と `/cache/` の正確な規則行があるかを確認する。
コメントや似たパスを部分文字列で照合すると、取込みとホスト開始で不足規則の追加を省略してしまう。
`store-gitignore` は両入口と既存テキスト保持を検査する。任意の否定規則による上書きの解析は対象外である
（[ADR 0031 の後続修正](../decisions/0031-content-addressed-store.md#除外規則の行単位の照合2026-10-29)）。

### 取込みを拒否したのに保管庫のファイルが増えた

`media/objects/` の名前で切り分ける。`.<hash>.<pid>-<seq>.tmp` は段階のまま残った一時ファイルで、その pid のプロセスが段階の途中で終わった。
64 桁の名前が増えたなら、その要求は受理されたか、ジャーナルの耐久性が不明で行が残り得る（[ADR 0272](../decisions/0272-staged-import-media.md)）。
拒否された取込みでハッシュの名前・`media/index.json`・`cache/peaks` が増えるのは回帰で、`audit_ext02_*` と `host_tests::an_import_keeps_*` が検出する。
Undo や削除で参照が消えた素材が残るのは仕様である。保管庫の掃除は未実装（[ADR 0031](../decisions/0031-content-addressed-store.md)）。

### 保存して開き直すと小さいゲインやパンが変わる・0 になる

編集直後の値と、`tracks/<id>.json` の数値、再読込後の Float のビットを比較する。
旧 formatter の小数点以下 9 桁への丸めが原因なら、ジャーナルでは値を保持してもパッケージで失われる。
`package-float-roundtrip` でゲインと負のパン、`clip-drag-identity-ui` で直交ドラッグ・原点復帰と保存を切り分ける。
前者の `autosave`・`incremental autosave` は保存直後、`commit recovery` は作業ファイルを古くした後のコミット読出し、
`history restore`・`restore undo`・`restore redo` は履歴復帰の各境界を示す。失敗名から formatter・増分キャッシュ・復帰を分けて調べる。
正確な旧表記は維持し、損失がある時だけ往復可能な表記へ変える。既に丸められた値は復元しない
（[ADR 0196](../decisions/0196-lossless-package-floats.md)）。

### パラメーターを含むパッケージの読込みが拒否される

`track json:` の必須項目・型・未知の項目・`invalid id`・`unknown parameter` は `Parameter` の形の復号で調べる。
`outside its range` は組立て後の制御の順序・端点・既定値と値の検証であり、復号の成功はプロジェクトの有効性を保証しない。
`parameter_package.rs` は各段階の拒否と原本の保持、旧スキーマ・保存のキー順とコミットからの再読込みを検査する
（[ADR 0222](../decisions/0222-parameter-package-fields.md)）。

### デバイスを含むパッケージの読込みが拒否される

`track json:` の必須項目・型・未知の項目・`invalid id`・`unknown device kind` は `Device` の形の復号で調べる。
`mixer state must be null` と `does not match its plugin id` は履歴の組立て前の検査である。
状態オブジェクトの欠落・`does not match its BLAKE3` は保管庫の検査で、モデルの直接復号は代わりにならない。
ピン・フラグ・順序・ID の重複は組立て後の不変条件と区別する。
`device_package.rs` で拒否と原本保持、旧版・保存のキー順と改行・コミット再読込みを検査する
（[ADR 0223](../decisions/0223-device-package-fields.md)）。

### ノートを含むパッケージの読込みが拒否される

`content json:` の必須項目・型・未知の項目・`invalid id` は `Note` の形の復号で調べる。
`articulation is not accepted yet` と `expressions are not accepted yet` は組立て前の未対応機能の拒否である。
`not usable`・`not in start, pitch, id order`・`id is already used` はプロジェクトの組立てで調べる。
直接の Serde は任意の整数・参照を表せても、音高・長さ・順序・ID の一意性を保証しない。
`note_package.rs` は形と意味の拒否・原本保持、保存バイト・旧版・共有内容・コミット再読込みを分けて検査する
（[ADR 0224](../decisions/0224-note-package-fields.md)）。

### トラックを含むパッケージの読込みが拒否される

`track json:` の必須項目・型・未知の項目・親の `invalid id` は `TrackMetadata` の形の復号で調べる。
`does not contain that id` はファイル名と同一性の照合、子の ID・中身・ミキサーの拒否は子の組立てと区別する。
`track is not in the project`・`cannot contain itself`・`track order key` は親・階層・兄弟順の検証である。
メタデータの直接復号や子との結合だけでは有効なプロジェクトにならない。
`track_package.rs` は形・同一性・子と階層の拒否、原本保持・保存バイト・旧版・部分木の履歴を検査する
（[ADR 0225](../decisions/0225-track-package-fields.md)）。

### 項目表の変更後に保存・ジャーナル・スナップショットが一致しない

モデルの `*_fields.rs` と投影指定、`changeset.rs` のプロパティ名、`change_values.rs` の値の codec を確認する。
保存の未知の項目・型は `project.json:` / `content json:` / `track json:` の形の復号で拒否する。
配置の不正 ID・レーン・種類も `track json:` の文脈を持ち、組立て後の範囲・参照の拒否と区別する。
履歴の `schema_fields.rs` と `change_value_wire.rs` は保存バイト・旧 wire 表記・異常入力と原本保持を検査する。
ABI の値だけが違う場合は FFI の `snapshot_fields_tests.rs` と `Snapshot::source_track_index`、元ハンドルの寿命を確認する。
子の解決・不変条件の検証を迂回して値の一致だけを合格にしない。新しいログや収集先はない
（[ADR 0226](../decisions/0226-model-field-registry-completion.md)）。

### 編集を重ねるとジャーナルの追記前確認が遅くなる

原本をコピーして保全し、`journal.jsonl` のバイト数と、末尾が改行で終わるかを記録する。中身を診断へ自動収集しない。
`document.rs` の `repair_journal` は通常は 1 バイトだけを読み、未完の末尾があれば 4 KiB ずつ後ろへ探す（[ADR 0252](../decisions/0252-bounded-journal-tail-repair.md)）。
改行のない長い末尾はその長さに比例して読むが、完全なログの全件読込みは不要である。
`cargo test -q -p echode-history --lib document::journal_tests` は製品の追記経路で読取バイト数と修復後の復旧を確認する。
これは経過時間の計測ではない。1 編集ごとの `File::sync_all`（macOS の `F_FULLFSYNC`）の待ちや、計画の全体再生成（FCA-04）とは分ける。
完結した不正行は消さず、次の復旧の節で調べる。同期を省略したり、原本のジャーナルを手で短縮したりして応答を速めない。

### 開き直すと確定済みの編集が消えている・古い状態で開く・開けない

警告または C ABI の `historyReport.recovery` の `tip`・`base`・`applied_seq` で、先端からの後退を確認する。
`DocumentHost::recovered_label` は従来どおり、オープン時の基点と復旧済み連番を返す。
先端が読めない原因は、オブジェクトの破損と、不変条件を破ったモデルのコミット（読込みで拒否される）を区別する。
破損ジャーナルは `journal seq <連番> violates model invariants: <理由>` でオープンを拒否する（[ADR 0180](../decisions/0180-journal-replay-invariants.md)）。
適用自体の失敗は `journal seq <連番> cannot apply changes: <理由>` と区別する。
`tests/parameter_ownership.rs` は、未列挙のパラメーターを復元時に捨てる経路を調べ、適用後の不変条件検査だけでは発見できない消失を検査する。
`changeset::Draft::finish` の全体 ID 集合・所有者・使用数を確認し、原本を保全したまま該当する行の作成・削除と `param_order` を照合する。
`audit_adv01_*` は通常回帰で、先端なし・ありと、明示修復後に確定した編集の保持を検査する。
原本をコピーして保全し、連番・不変条件の理由・基点と先端を記録する。自動で行を削除したり、繰り返し保存して直そうとしない。
旧版では不正な状態をコミットし、debug では最初の編集が panic する経路もあった。
後退したホストは `recovery confirmation required` で書込みを拒否する（[ADR 0183](../decisions/0183-recovery-consent.md)）。
`recovery.reason` が `journal_gap` ならジャーナルの連番の欠け、`missing_state` なら再生する行の名指す状態の保管物の欠落・破損で、
どちらも `tip` と `base` は同じになる。参照だけが古い時はコミットチェーンの最新から開き、通知しない（[ADR 0230](../decisions/0230-save-durability.md)）。
保存の同期の順序は `cargo build -p echode-cli` の後に `python3 -B scripts/durability_trace.py --require-durable` で確かめる。
警告でいったんキャンセルし、原本のコピーを保全する。「続行」は復旧状態の同期保存であり、失われた編集の復元ではない。
確認時の保存に失敗した場合は編集を解放しない。CLI / MCP には同意の入口がないため、アプリで確認する。
確認の取消後も開く処理が残る場合は、要求の UUID と `RecoveryOpenRequest` の取消・`recoveryReply` の解放を調べる。

### 起動直後から新規作成・開くが無効で「変更を保存中」のまま

前回プロジェクトの自動復元と `ProjectController.busy` を確認する。
`sample` の読取で復元側が `Autosave::drop` の join、`echode-autosave` が初期化失敗後の受信待ちなら、終了要求を無視する旧不具合と一致する。
初期化失敗中も `Shutdown` で終了し、復元エラーとパッケージロックの解放を返す必要がある。
`startup-recovery` は子プロセスの停止防止期限でこの循環待ちを検出し、Swift の `failedStartupOpenReenablesNewAndOpen` はエラー後の新規作成を確認する。
復元対象の削除や前回パスの初期化で問題を隠さず、エラーと Git の状態を切り分ける。

### 保存・復旧・Undo / Redo・MCP

バックグラウンドのプラグイン状態保存は Undo に積まず Redo も消さない（[ADR 0109](../decisions/0109-placement-bounds-and-plugin-capture.md)）。
CLI 書出しは `DocumentHost` の復旧スナップショットを使う。
テストで既存パッケージの JSON だけを上書きしても保存済み履歴は置き換わらない。文書の編集経路を使うか、別の入力は新しいパッケージへ書く。
MCP（`echode mcp SOCKET`）は接続先の `echode serve PACKAGE SOCKET` と同じビルド・絶対ソケットパス・API 版 1 を確認し、stdout へログを混ぜない（[ADR 0115](../decisions/0115-phase2-mcp.md)）。

## MCP

### MCP が `request id must be a string or integer` を返す・通知の応答が来ない

`mcp.rs::McpSession::reply` の ID 検証と通知の早期 return を確認する。
MCP は null・真偽値・配列・オブジェクト・小数の ID を拒否する。null が正当な JSON-RPC API とは異なる。
ID の省略は通知として応答せず、`tools/call` を装っても接続・編集しない。
`mcp_requests.rs` と必須契約 `mcp-request-identity` は単一／一括編集・Undo / Redo の未送信と履歴保持を検査する。
正当な文字列・整数 ID まで拒否される場合は、元の型と旧・新 MCP の初期化・メタデータを分けて照合する
（[ADR 0115 の入力回帰](../decisions/0115-phase2-mcp.md#要求-id-と通知の回帰補強2026-10-29)）。

### 旧 MCP が `initialize first` / `already initialized` / `request id was already used` を返す

`initialize` の成功だけでは編集可能にならない。続く ID なしの `notifications/initialized` が必要である。
初期化前の通知、null・配列等の params、ID を持つ同名の要求は準備完了へ遷移させない。
使用済み ID は型も照合し、数値 `1` と文字列 `"1"` を混同しない。dispatch 済みの失敗にも新しい ID を使う。
`mcp_lifecycle.rs` と必須契約 `mcp-legacy-lifecycle` は同じ stdio と実ソケットで、未送信・履歴・再オープンを検査する。
ここでの拒否は接続先 API の版エラーや、編集後の結果不明とは異なる
（[ADR 0115 の初期化回帰](../decisions/0115-phase2-mcp.md#旧-mcp-の初期化と-id-再使用の回帰補強2026-10-29)）。

### 新 MCP が `unsupported protocol version` / `clientCapabilities must be an object` を返す

接続先 API の版ではなく、各要求の `params._meta` の MCP 版と capabilities を確認する。
版のキーを持つ要求は旧 MCP の準備完了に関係なく `validate_modern` を通り、未知・旧版の文字列は `-32022`、型の不正は `-32602` になる。
直前の正当な新 MCP 要求からメタデータを引き継がず、初期化していない旧 MCP の入力も編集を解放しない。
`mcp_metadata.rs` と必須契約 `mcp-modern-metadata` は接続前の拒否・履歴保持・後続の正当な編集と再オープンを検査する
（[ADR 0115 のメタデータ回帰](../decisions/0115-phase2-mcp.md#新-mcp-の要求ごとのメタデータの回帰補強2026-10-29)）。

### MCP のツール引数が拒否される・拒否したのに履歴が変わる

`mcp.rs::call_tool` はツール名からアクションの `type` を決める。呼出側の `type` は同じ値・null でも拒否する。
Undo / Redo・概要は引数なし、一括編集は `actions` だけを受ける。省略した arguments と明示した null は区別する。
型の不正・未知のツールは `-32602`、既知のツールの余分な引数は `isError` の実行エラーになり、どちらも接続しない。
`mcp_arguments.rs` と必須契約 `mcp-local-arguments` は接続数・ジャーナル・既存 Redo・後続の編集と再オープンを検査する。
接続先で拒否したアクションや結果不明とは段階が異なり、この拒否では自動再送や履歴の巻戻しは不要である
（[ADR 0115 の引数回帰](../decisions/0115-phase2-mcp.md#ツール引数のローカル拒否の回帰補強2026-10-29)）。

### MCP が `unsupported Echode session protocol` を返す

MCP 自体のプロトコル版ではなく、接続先 API の `protocol.version` が返す整数の版 1 を確認する。
この段階での拒否では編集をまだ送っていない。編集後の応答破損や切断による結果不明と区別する。
`mcp_version.rs` と必須契約 `mcp-backend-version` は、実ソケットで未送信・履歴保持・再オープンを確認する。
正しい版 1 の追加フィールドは拒否しない（[ADR 0115 の版確認](../decisions/0115-phase2-mcp.md#接続先の版確認の回帰補強2026-10-29)）。

### MCP が `invalid session response` と結果不明を返す

接続先の版・ID・result/error の排他と、error の整数 code・文字列 message を照合する。
`error:null` やフィールドの欠落・型違いは通常の実行エラーではなく不正な応答である。
編集が既に記録済みの可能性があるため、概要を確認せずに再送しない。原本の編集を自動で巻き戻さない。
`mcp_responses.rs` と必須契約 `mcp-backend-errors` は、実際の編集後に応答だけを壊し、
結果不明の案内・再送禁止・単一 Undo / Redo と再オープンを検査する
（[ADR 0115 の応答検証](../decisions/0115-phase2-mcp.md#接続先のエラー応答の形の検証2026-10-29)）。
応答の ID は要求した整数と同一でなければならず、文字列・小数へ読み替えない。
外枠は `mcp-backend-envelope`、版応答の外枠は `mcp-version-envelope` で別々に検査する。
版確認中の外枠拒否なら編集は未送信だが、編集後の拒否なら既に確定している可能性がある。
同じ保守的な結果不明の案内でも、どの段階の応答かを分けて調べる
（[ADR 0115 の外枠回帰](../decisions/0115-phase2-mcp.md#接続先の応答外枠の回帰補強2026-10-29)）。
完全な応答を送った後、検査用サーバーの `shutdown(Write)` だけが `NotConnected` になる場合は、先に正常に閉じた相手との競合を調べる。
`mcp_responses.rs` は半切断せず後続の読込みで EOF と再送禁止を検査する。部分行の拒否に必要な半切断とは区別する。

### MCP 改行回帰で `backend EOF wait` が 5 秒の期限切れになる

2026-10-30 の検査基盤改定で、`mcp_framing` の版確認中・部分応答後の切断待ちが全体・単独とも一度ずつ失敗した。
後の同じ構成の単独実行では再現しなかった。子プロセスの起動前ではなく、部分応答後の EOF 待ちという別の境界である。
`handshake`・`suffix`・`following` と両側のスレッドを調べ、走査ヘルパーの起動遅延と同じ原因だと断定しない。
期限・期待値・検査対象は維持し、失敗と再実行を時間履歴・個別ログで区別する。根因は未確定である。

### MCP が `MCP message ended without a newline` を返す

stdio 入力と接続先 API の応答は同じ `mcp.rs::read_line` を通るため、どちらの EOF かを切り分ける。
stdio の部分行は編集前に拒否し、完了済みの前方の要求だけを保持する。CR だけの終端では完了しない。
版応答の部分行では編集を送らないが、編集応答の部分行では編集が既に成立している可能性がある。
`isError` と結果不明の案内を確認し、概要を調べずに再送しない。
`mcp_framing.rs` と必須契約 `mcp-stdio-framing`・`mcp-backend-framing` が両境界を実ソケットで区別する
（[ADR 0115 の回帰補強](../decisions/0115-phase2-mcp.md#改行境界の回帰補強2026-10-29)）。
2026-10-29 のエラー応答検証では、通常の `mcp-backend-framing` が部分応答後の EOF 待ちで 2 回期限切れとなった。
`read_to_string` が 5 秒後に `WouldBlock`（Darwin errno 35）を返したもので、応答形のアサーション失敗ではない。
追加した診断では初回の版応答（`handshake=true, suffix=""`）後で、受信済みの `following` は空だった。
単独実行と正式な再実行では再現しない場合もあり、原因は未確定である。期限や対象は変更しない。
次回も診断の `handshake`・`suffix`・受信済みの `following` で版応答と編集応答を切り分ける。
単独の記録は `target/focused-checks/mcp-framing-timeout/`、正式な再実行は検査レポートで確認する。
