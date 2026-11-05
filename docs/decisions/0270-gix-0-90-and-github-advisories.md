# 0270. gix を 0.90 へ上げ、GitHub の勧告は RustSec と別に確かめる

- 日付: 2026-11-05
- 状態: 採用
- 関連: [ADR 0005](0005-s2-confirmed-s4-next.md)（gix の採用）、[ADR 0046](0046-history-thin.md)（`gix-pack` による間引き）、
  [ADR 0230](0230-save-durability.md)（保存の同期）、[ADR 0269](0269-github-public-ci-scope.md)（GitHub の CI）

## 背景

GitHub へ公開した最初のプッシュで、Dependabot が gitoxide の勧告 7 件（アラート 8 件）を報告した。
同じコミットで `make advisories`（`cargo deny check advisories`）は合格した。
当時の版は `gix` 0.78.0、`gix-pack` 0.65.0、`gix-fs` 0.19.2、`gix-transport` 0.53.0 である。

## 各勧告と Echode からの到達

Echode が呼ぶ gix の API は、`gix::init` / `gix::open`、オブジェクトと blob の書込み、`commit_as`、
コミットとオブジェクトの読出し、参照の読出しと更新だけである（`crates/echode-history`、2026-11-05 に照合）。
サブモジュール、チェックアウト、状態の比較、リモートとの通信は呼ばない。

| 勧告 | 重さ | 対象と修正版 | 到達 |
| --- | --- | --- | --- |
| GHSA-x494-mj8g-cj27 | high | `gix-pack` ≤ 0.68.0 → 0.69.0 | 一部到達する（下記） |
| GHSA-f89h-2fjh-2r9q | high | `gix-fs` ≤ 0.21.0 → 0.21.1 | 到達しない。gix でチェックアウトしない |
| GHSA-fr8x-3vfx-f45h | high | `gix` < 0.83.0 | 到達しない。サブモジュールの API を呼ばない |
| GHSA-pg4w-g64p-qwhj | high | `gix` < 0.83.0 | 同上（`Repository::submodules()`） |
| GHSA-f26g-jm89-4g65 | high | `gix` 0.31.0〜0.82.x | 同上（`Submodule::update()`） |
| GHSA-p3hw-mv63-rf9w | high | `gix` < 0.83.0、`gix-validate` ≤ 0.10.0 | 同上。`gix-validate` は 0.11.4 で対象外だった |
| GHSA-9857-6mw7-fq2m | medium | `gix-transport` 0.25.4〜0.55.0 → 0.56.0 | 到達しない。curl の HTTP 経路は機能が無効で、ビルドされない |

GHSA-x494 は、細工したパックのデルタで添字の範囲外の panic を起こし、サイズの見出しで上限のない確保を起こす。
勧告は clone と fetch を経路に挙げるが、Echode は通信しない。
ただし `gix::open` したリポジトリの `.git/objects/pack` を読む時も、同じ復号の経路を通る。
Echode が書くパックはデルタを持たない（ADR 0046）。
それでも、他人から受け取ったプロジェクトや壊れたパッケージを開くと、履歴のスレッドや開く処理が panic か確保の失敗で止まりうる。
情報の漏れや書込みはなく、影響はサービス拒否に限る。

GHSA-m4f9-c775-wg56 は GHSA-f26g の重複として取り下げられている。

## 決定

- workspace の `gix` を 0.90.0（2026-10-22 公開、最新）へ上げる。
  0.83〜0.89 にしないのは、0.88.0〜0.89.0 に GHSA-q54r-jf2p-rfr2（high、信頼の低い `.git/config` が `.git` の保護を外す）があるためである。
- `gix-pack` の固定は、gix が解決する 0.77.0 へ合わせて `=0.77.0` にする。
- 新しい `Cargo.lock` の crates.io の 318 crate 全部を、GitHub の勧告データベースへ `affects=` で照会した。
  該当はなかった（2026-11-05）。対照として旧版の gix 系 4 crate を照会すると、上の 7 件と取り下げ済みの 1 件を返した。
- API の変更に合わせ、間引き（`crates/echode-history/src/thin.rs`）を直した。
  `Bundle::write_to_directory` はハッシュの種類を取るので `repo.object_hash()` を渡し、パック末尾の検査値も同じ種類で計算する。
  `Find` の誤りの型が `gix-error` の型に変わったため、thin pack の参照先は自前の空の実装をやめ、`gix::objs::find::Never` を使う。

### `gix-pack` を直接の依存にして版を固定する理由

`gix-pack` は ADR 0046 の `979dbc4` で足した。

- gix は `gix-pack` を既定の機能なしで使う。間引きが呼ぶ `Bundle::write_to_directory` は、`gix-pack` の `streaming-input` 機能の中にある。
- `echode-history` が既定の機能つきで直接依存すると、Cargo の機能の統合で、gix が使う同じ `gix-pack` にこの機能が入る。
- 版が gix の解決と違うと `gix-pack` が 2 つ入り、gix の型と `gix-pack` の引数の型が別の版になって、ビルドが通らない。
- `=` の完全一致は、gix を上げずに `gix-pack` だけが動くことを防ぐ。
  gix を上げる時は `cargo update -p gix -p gix-pack` を実行し、解決した `gix-pack` の版へ固定を合わせる。

### 依存とライセンス

gix 系の crate は MIT OR Apache-2.0 のままである。
新しく入った推移的な依存は `dua-core`（MIT）、`nonempty`（MIT）、`gix-imara-diff`（Apache-2.0）、
`sha1dc`・`crossbeam` 一式・`gix-zlib`・`gix-parallel`・`gix-macros`・`gix-merge`・`gix-note`（MIT OR Apache-2.0）で、
すべて GPL-3.0-or-later と両立する。`make check` の `cargo deny check licenses` も照合する。

### 同期の前提

ADR 0230 は「gix は同期を呼ばない」を前提に、Echode 自身がオブジェクトと参照を同期する。
新しい `gix-odb` 0.87.0、`gix-ref` 0.69.1、`gix-tempfile` 26.0.1、`gix-lock` 26.0.1、`gix-pack` 0.77.0、`gix-fs` 0.24.1、`gix` 0.90.0 のソースにも、
`sync_all`・`sync_data`・`fsync`・`F_FULLFSYNC` の呼出しはない。Echode 側の同期は引き続き必要である。

## `make advisories` が合格し、Dependabot が報告した理由

- `make advisories` は cargo-deny で、RustSec の advisory-db だけを照会する。`deny.toml` に `[advisories]` の除外はない。
- 照会した RustSec の DB（コミット `965bf9a`、2026-11-03）は、gix 系では 2025 年までの 14 件だけを持つ。
  今回の 7 件の GHSA は、別名（`aliases`）にも無い。
- 7 件は gitoxide のリポジトリの security advisory として 2026-05-05〜07 に公開され、GitHub Advisory Database に入った。
  RustSec へは、取り込みの PR が出るまで入らない。2026-11-05 の時点で、RustSec のリポジトリに取り込みの issue・PR は無い。
- したがって `make advisories` の合格は、RustSec に載った勧告が無いことだけを示し、GitHub の勧告が無いことは示さない。
  依存を更新した時と Dependabot が新しいアラートを出した時は、
  `gh api 'repos/crab-hoo/echode/dependabot/alerts?state=open'` で確かめる（[デバッグ手順](../debugging/verification-and-build.md)）。

## 回帰・検査基盤

- 既存の `echode-history` のテスト（間引きの `tests/thin.rs`、保存の耐久 `audit_durability`、復旧 `audit_recovery` など）と、
  `echode-ffi` の復旧テストが新しい gix で合格した。gix の内部の版が合わなければ、ビルドの段階で止まる。
- `make spike-history` も合格した。1,000 トラックの初回コミットは 640 ms、前の版の読み直しは 86 ms で、1 秒以内の基準を満たす。
  [S4](../spikes/S4-history.md) の記録（323 ms、27 ms）より遅いので、変更前の HEAD（gix 0.78）を同じ時間帯に交互に 5 回ずつ測った。
  負荷平均 10〜18 の中で、初回コミットの中央値は旧 528 ms・新 505 ms、読み直しは旧 63 ms・新 66 ms だった。
  差は版ではなくマシンの負荷によるとみて、S4 の記録は書き換えない。
- GitHub の勧告を照会する自動検査は足していない。`make advisories` の範囲は `docs/verification.md` に明記した。
