# ADR 0232: 検査の選択と意図した red の照合の取りこぼしを塞ぐ

- 日付: 2026-11-02
- 状態: 採用
- 関連: [ADR 0212](0212-verification-tiers.md)、[外部監査の精査](../reviews/2026-11-02-external-audit-review.md)の ENV-01・02・05・06・07、
  [実行指示](../reviews/2026-11-02-fix-instructions.md)の W4

## 背景と再現

開始時の HEAD は `58ee664`。

- ENV-01: 作業の段階でサニタイザを選ぶ `unsafe_changes` は、存在しないファイルを読み飛ばした。
  `unsafe` を含むファイルを消してもサニタイザが選ばれず、[検査手順](../verification.md)の「変更行（追加・削除）」と食い違っていた。
  C の変更も対象外だった。
- ENV-06: `checked_commands.py --expect-failure` はログ全体を照合した。ログの先頭の `Command:` 行は argv を写すので、
  正規表現が引数に現れるだけで、何もしない終了コード 101 が「期待した失敗」になった。
- ENV-05: `check-ffi.sh` は `unsafe extern {`（ABI なし）、`extern "system" {`、波括弧が次の行の宣言を見逃した。現在の違反は 0 件。
- ENV-07: 文書のリンクの検査は `file://` を外部のリンクとして飛ばした。

`test_agent_hooks.py` の ENV-01・ENV-06 の再現は、`skip` を外すと固有のアサーションで失敗した。

## 決定

- `unsafe_changes` は消したファイルも差分の基準から削除行を読み、脱出口を数える。差分の基準がなければ、消したことだけで選ぶ。
- `.c`・`.h` は対象にする。C 自体にはメモリと競合の検査がないので、空行以外の変更行はすべて脱出口として数える。
  対象は従来どおり、その検査の入力の範囲で、テストのディレクトリを除く。
  現在の該当は橋渡しの `swift/EchodeBridge/CEchodeFfi/shim.c` と `spikes/bridge/sanitizers/Errno.c` である。
- `checked_commands.py` は、ログの先頭のヘッダ（`Command:`・`Timeout:` と空行）を除いた、コマンドが出力した部分だけを照合する。
  コンパイルの出力など、テストの開始前の行は区別できないため照合に残す。正規表現はその回帰に固有の文にする（従来どおり）。
- `check-ffi.sh` は、行頭の `extern` に任意の ABI 文字列が続くか続かず、同じ行に `{` があるか行が終わるものを宣言とみなす。
  `extern "C" fn` の定義と `extern crate` は含めない。
- 文書のリンクの検査は `file:` の URL を誤りにする。1 台のパスを指し、検査もされないため、相対リンクを使う。
- `audit_plg9_a_watch_can_be_dropped_right_after_it_starts` は、3 秒の期限を監視の破棄だけにかける。
  開始は 60 秒の上限で、止まった時だけ止める。
  この日の W1〜W4 の検査で `check` がこのテストだけで 8 回落ち、標本はいつも開始側の fseventsd への登録待ちだった。
  守る対象（開始直後の破棄が止まらないこと、PLG-9）と破棄の期限は変えていない。

## 残る範囲

- ENV-02: 全体の段階の検査の `covered`（前回の合格が見た変更）は、検査の定義やツールの識別値を見ない。
  定義が変わった後も、前回の合格が見たファイルを理由に作業の段階で選ばれないことがある。
  到達には、検証を飛ばした中間の版が要る（監査の判定）。直すには選択のたびにツールの識別値を計算する必要があり、今回は見送った。
- ENV-03・ENV-08 は監査の方針どおり扱いを変えない。

## 回帰と基盤

- `test_agent_hooks.py` の ENV-01・ENV-06 の `skip` を外した。
- `test_a_changed_c_line_selects_the_sanitizers_and_tests_do_not` は C・ヘッダの変更の選択と、テストのディレクトリの除外を確かめる。
- `ForeignDeclarations` は 3 つの宣言の形を検出し、定義を含めないことを確かめる。修正前の `check-ffi.sh` では何も検出せず失敗した。
- `test_diagnostics.py` の `test_file_url_links_are_rejected` は `file:` のリンクを誤りにすることを確かめる。
- `scripts/verify.py` の変更は検査の識別値に入るので、全自動検査が選ばれる。
