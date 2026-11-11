# ADR 0320: パッケージの項目の拒否を型にし、文字列のエラーの検査をワークスペース全体へ広げる

日付: 2026-11-11
状態: 採用
関連: M1（DEV-7・SPG-11・XCA-16）、[ADR 0317](0317-typed-control-transport-errors.md)、[ADR 0311](0311-typed-shared-memory-errors.md)、[ADR 0291](0291-typed-audio-io-errors.md)

## 問題と再現

開始時の HEAD は `241432c`。M1 の残りは、ADR 0317 が挙げた `echode-history` の文字列リテラル 4 か所と、検査の範囲だった。

- `objects.rs::read_object` は、`media`・`objects` がない時に `NotFound` の `media is missing`・`objects is missing` を返した。
- `package_io.rs` は、書込みで開いたファイルが通常でないか他のハードリンクを持つ時に `package file is not a regular unshared file`、
  名前が 1 つの通常の要素でない時に `package entry must be a single path component` を返した。
  リンク数・実際の種類・名前は文の中にも残らなかった。
- 同じファイルの `require_kind` は、名前と「シンボリックリンク」「種類の違い」を `format!` で 1 つの文へ畳んでいた。
  ADR 0311 が「入力名などの説明の整形は許す」としたので、検査は見つけなかった。
- `check_typed_errors.py` は登録した 11 crate だけを見た。登録のない 7 crate（`echode-draw`・`echode-dsp`・`echode-fuzz`・
  `echode-realtime`・`echode-sine-pad`・`echode-test-plugin`・`echode-time`）と `spikes/bridge` は、
  文字列のエラーを足しても止まらなかった。新しい crate も、誰かが一覧へ足すまで対象外になる。

棚卸しで、ワークスペースの全メンバーに全規則をかけると、違反は上の 4 か所だけだった。
`format!`・`.to_string()` を I/O エラーへ渡す所は、`require_kind` のほかになかった。
Rust の製品コードと Swift のアプリに、エラーの文面で分岐する所は見つからなかった（`contains`・`starts_with` 等の grep）。

公開の入口から回帰を先に追加した。`read_object`（`media`・`objects` の欠落）、`append_journal`（ハードリンクのジャーナル）、
`read_package` と `DocumentHost::open`（シンボリックリンクのトラック、ディレクトリの `project.json`、ファイルの `tracks`）である。
修正前は 3 件とも、表示文の照合を通った後の `must retain its typed cause`・`must retain its typed link count` で失敗した（終了コード 101）。
記録は `target/focused-checks/package-entry-red/` に残す。

## 比べた案と決定

1. 文を残し、検査の対象から `echode-history` を外す。リンク数・種類・名前が文へ消え、全 crate への拡大もできないので採らない。
2. `PackageError` に変種を足す。項目の拒否は `Directory` の 14 の操作が返す `io::Result` の内側で起き、`Io { path }` のパスと一緒に届く。
   `Directory` の結果の型と、`io_error` で包む 65 か所の呼出しを変えることになり、OS の失敗と拒否が別の経路に分かれるので採らない。
3. **`PackageEntryError` を `io::Error` の内側の型にする。種類（`ErrorKind`）と表示文は従来のまま、名前・種類・リンク数を `get_ref` で読める。**

3 を採る。ADR 0317 の `DescriptorRuleError` と同じ形である。

- `MissingFolder { name }`（`NotFound`）は `Directory::required_child` が返す。`read_object` はこれで `media`・`objects` を開く。
- `NotSingleComponent { name }`・`WrongType { name, actual, expected }`・`NotRegularUnsharedFile { kind, links }` は `InvalidInput` である。
- 種類は `EntryKind`（`File`・`Directory`・`SymbolicLink`・`Other(形式のビット)`）で、モードの `S_IFMT` から一か所で求める。
- 拒否の判定・順番・削除しない動作（PRO-01）・表示文・保存と復旧の判断は変えない。音声処理・C ABI は変えず、性能改善は主張しない。

### 検査をワークスペース全体へ広げる

`check_typed_errors.py` は `Cargo.toml` の `[workspace] members` を読み、全メンバーの `src` に全規則をかける。
一覧（`TYPED_CRATES`）と部分移行の仕組み（`TYPED_FILES`）をなくした。新しい crate は、ワークスペースへ足したコミットから対象になる。
`members` の glob は展開し、`Cargo.toml` やメンバーの `src` がなければ失敗として報告する。メンバーでない `spikes/draw` などは対象外である。

- 文字列リテラルだけの I/O エラー（ADR 0317）を全メンバーで拒否する。種類が原因そのものの `TimedOut`・`UnexpectedEof`・`WriteZero` は許す。
- I/O エラーへ `format!`・`.to_string()` を渡すことを、種類を問わず拒否する。ADR 0311 の「入力名の整形は許す」例外はなくした。
  使う所がなくなり、残すと名前や値を文へ畳む書き方を検査が通すためである。
- `PackageError::new`・`Message` の拒否は `echode-history` だけに残す。その型が `echode-history` にしかないためである。

`spikes/bridge` も、ワークスペースのメンバーなので対象に含める。修正前から違反はない。

## 回帰と基盤

- `tests/package_entries.rs` は、公開の入口で `io::Error` の種類・内側の型・従来の表示文を照合する。
  エラーの連鎖をたどり、`io::Error::source` が自分の内側を飛ばすので、各段で `get_ref` も読む。
- `package_io.rs` の単体回帰は、`a/b`・`..`・`.`・絶対パス・空の名前と、FIFO（`Other(S_IFIFO)`）を照合する。
- `scripts/tests/test_typed_errors.py` は、全メンバーでの各規則、一覧なしで新しいメンバーを検査すること、glob の展開、
  メンバー外のフォルダを対象外にすること、`Cargo.toml`・`src` の欠落の報告を表で固定する。
  修正前の HEAD の木に新しい検査をかけ、4 か所を `gives an I/O error only constant text` で見つけることを手で確かめた。
- `check` の検査の選択と Makefile は変えない。`Cargo.toml` は全検査の共通入力に既にある。
- コードの所在、検査の診断手順、監査の状態を更新する。利用者の制約・実機の確認場面は増えず、status.md とユーザー確認の一覧は変えない。

## M1 の完了

DEV-7 の完了条件「UI が使う境界のエラーが列挙型になっている」と、SPG-11 の方針「文字列化は FFI と表示の直前だけ」を満たした。

- ワークスペースの全 crate で、`Result<_, String>`・原因の文字列化・文だけの I/O エラーを静的に拒否する。
- 文字列にするのは C ABI の完了（`echode-ffi` の `finish`・目録）、API の wire 応答、CLI の表示だけである。
  Swift のアプリは文面で分岐せず、表示する。
- `String` を持つ残りの変種は、利用者の入力（CLI の `WavFormat`・`Dither`）、窓の題名、panic の文、ID・パスの写しで、原因を畳んだものではない。

統合表の M1 を対応済みとし、ADR 0278 の順で次は M2（`unsafe` の根拠と FFI の長さの検査）である。
検査は構文で見るので、変数に入れた `&'static str` や、型の推論を要する経路は見つけない（ADR 0317 と同じ）。
検査結果と各検査の見送り理由は `target/verification/report.md` を正本とする。
