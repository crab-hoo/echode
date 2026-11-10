# ADR 0311: 共有メモリの失敗で操作と errno を保持する

- 日付: 2026-11-10
- 状態: 採用
- 対象: M1（DEV-7・SPG-11・XCA-16）、`echode-sandbox`
- 前提: [ADR 0310](0310-typed-plugin-load-boundary.md) の棚卸し

## 問題と再現

`region.rs::shm_file` は `shm_open`・`shm_unlink`・`ftruncate` の失敗を
`io::Error::other(format!(…))` にしていた。
操作は表示文にだけあり、元の errno と `ErrorKind` は失われ、`RegionError::source` もなかった。
`map_err` の外なので、既存の再導入の検査は検出しなかった。

製品の `MappedRegion::create_shared` に、mapping 長は `usize` に収まるが
`off_t` では負になるレイアウトを渡す回帰を先に加えた。
実際の `ftruncate` が `EINVAL` で拒否し、ページはマップしない。
修正前は `shared creation must retain the native errno` のアサーションで失敗した。
静的検査にも、`map_err` の外の I/O エラー構築を検出する反例を先に加えて失敗を照合した。

## 比べた案と決定

1. 表示文だけを維持する。操作・errno の分類に文の解析が必要なので採らない。
2. `io::Error` の内部へ独自の文脈型を包む。型は保持できるが、操作を読むにも downcast が必要になる。
3. **`RegionError::SharedMemory { operation, error }` に分ける。**
   操作は `SharedMemoryOperation` の `Open`・`Unlink`・`Truncate`、原因は元の `io::Error` とする。

3 を採る。既存の `Io`・`Lock` と同じ境界で分類でき、原因を文へ戻さない。
表示は従来の `shm_open: …`・`shm_unlink: …`・`ftruncate: …` を保つ。
`Error::source` からも元の I/O エラーを返す。
`SessionError` への変換は既存の `Region` で保持する。
成功した FD は、後続の操作の前に `File` が所有し、失敗時も drop で閉じる。

## 回帰と検査の境界

- `region_error_tests.rs` に製品入口の実 `ftruncate`、既存名との実 `shm_open` の衝突、
  全 3 操作の変種・errno・表示文・source、成功時の unlink・サイズ・close-on-exec を置く。
- `shm_file_named` は生成名を受ける既存処理を分けた私有関数である。
  グローバルな故障注入や製品のオプションは増やさない。
- `shm_unlink` 自体の失敗は実 OS で故障注入せず、その変種の原因と表示を検査する。
  競争状態を作る不安定なテストや、3 システムコール専用の差替え層は採らない。
- macOS は共有メモリの長さをページ単位へ切り上げる。
  成功時は要求以上の長さを検査し、4096 バイトぴったりとは仮定しない。

`check_typed_errors.py` は登録済みの crate で `io::Error::other` の文への変換と、
`io::Error::new` 内の `.to_string()`・明示的な `last_os_error` の整形を検出する。
最初に `new(format!(…))` を一律に拒否すると、パッケージの入力名と種類の説明も拒否した。
これは既存の原因を消していないため対象外とし、反例と許容例を両方固定する。
検査は構文の防護であり、変数の型・別名・文の経路を推論して全件を証明するものではない。

通常の Rust・検査基盤のテストが新しい回帰も実行するので、検査の選択条件と入口は増やさない。
fuzz の共有領域の入力形式、IPC・C ABI、音の処理と実機の期限は変えない。
隔離の機器なし回帰を使い、実機・性能・聴感の新しい合格を主張しない。

## 残り

M1 全体の完了とはしない。
次は `control.rs` の窓名と `params.rs::Cursor::text` の不正 UTF-8 で、
元の復号の原因を捨てて定数の文だけの `io::Error` にする境界を扱う。
結果と検査ごとの見送り理由は `target/verification/report.md` を正本とする。
