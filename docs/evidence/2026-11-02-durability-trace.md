# 2026-11-02 保存の耐久性の追跡

[横断監査](../reviews/2026-11-02-cross-cutting-audit.md) の XCA-01〜03 の証拠の要約である。

## 対象と環境

- 対象: `b417ff4` と未コミット変更 34 件をビルドした `target/debug/echode` の複製。
  未コミット分は保存の経路（`crates/echode-history`）を変更していない。
- 実行は作業領域の外で行い、リポジトリのファイルは変更していない。
- 実機・音声機器・ウィンドウは使っていない。
- 実際の電源断は行っていない。下の S1〜S4 は、失われ得る状態を手で作ったものである。

## 手順

1. `fcntl(F_FULLFSYNC)`・`fsync`・`rename`・`renameat`・`link` を記録する動的ライブラリを作る。
   `__DATA,__interpose` で差し替え、呼出しの種類とパスを順に 1 行ずつ追記する。
2. `crates/echode-history/tests/fixtures/schema-1` を複製してパッケージとする。
3. `DYLD_INSERT_LIBRARIES` を付けて `echode serve PACKAGE SOCKET` を起動する。
4. ソケットへ次を 1 秒おきに送る。
   `track.add`（first）、`track.add`（second）、`track.add` と `audio.import` の一括（third）、`track.add`（fourth）。
   音声は `crates/echode-cli/tests/fixtures/sine-pad-a4-128.wav` を使う。
5. `SIGKILL` で止め、パッケージの複製を変更して、同じ CLI で開き直す。

## 追跡の結果

パスはパッケージからの相対で、ハッシュと ID を省略した。編集 3 の区間を示す。

```text
=== edit 3（直前の edit 2 を圧縮する）
FULLFSYNC .journal.jsonl.tmp
RENAMEAT  journal.jsonl
FULLFSYNC パッケージのディレクトリ
RENAMEAT  media/objects/<blake3>        同期なし
RENAMEAT  media/index.json              同期なし
RENAMEAT  cache/peaks/<hash>            同期なし
FULLFSYNC journal.jsonl                 保管物を参照する行
FULLFSYNC .save.pending.tmp
RENAMEAT  save.pending
RENAMEAT  project.json                  同期なし
RENAMEAT  tracks/<uuid>.json            同期なし
RENAMEAT  contents/<uuid>.json          同期なし
RENAME    .git/objects/*（7 個）        同期なし
RENAME    .git/refs/echode/autosave/main 同期なし
FULLFSYNC .git/echode-commit-chain
=== edit 4（edit 3 を圧縮する）
FULLFSYNC .journal.jsonl.tmp
RENAMEAT  journal.jsonl
FULLFSYNC パッケージのディレクトリ
```

全区間を通じて、`.git/objects`、`.git/refs`、`media/objects` への同期は現れなかった。
Cargo.lock の gix-odb 0.75.0 と gix-ref 0.58.0、gix-tempfile、gix-lock のソースにも同期の呼出しはない。

## クラッシュ状態の再現

終了後のジャーナルは連番 4 の 1 行、先端のコミットは連番 4 である。基準は `piano, first, second, third, fourth`。

| 状態 | 作り方 | 開いた結果 | 編集 |
|---|---|---|---|
| S1 | 参照を連番 2 のコミットへ戻す | 成功、`piano, first, second`。通知なし | 受け付ける |
| S2 | 先端のコミットのオブジェクトを消す | 成功、`third, fourth` まで復元 | （未試行） |
| S3 | 先端と親のコミットのオブジェクトを消す | 成功、`piano, first, second` | `recovery confirmation required` で拒否 |
| S4 | `media/objects/<blake3>` を 0 バイトにする | 成功、全トラック。通知なし | 下記 |

S4 の続きは次のとおりである。

- `echode render`: `echode: audio file could not be read`。
- 同じファイルの再取込み: `object <hash> already holds different bytes`（エラーコード -32000）。

## 判定の範囲

- 示したのは「同期していない」ことと「その内容が失われた時の製品の振る舞い」である。
- APFS で実際に失われる頻度と時間幅は測っていない。
- プロセスの異常終了だけでは OS のキャッシュが残るので、この経路の喪失は起きない。
- S1 の状態は、参照の名前変更が永続化しない場合に対応する。保管物の 0 バイトは、名前変更が先に永続化した場合に対応する。
