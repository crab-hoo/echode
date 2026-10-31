# 0207. 継続の先行再開の前後関係を TSan へ注釈で示す

- 日付: 2026-10-31
- 状態: 採用
- 関連: [ADR 0044](0044-swift-bridge.md)、[ADR 0121](0121-startup-recovery-and-errno-validation.md)

## 背景と原因

`check-bridge-tsan` が、`Snapshot.audioState()` の読取り（`Document.swift:59`）と、`completeCommand` の中の `Snapshot.init(taking:)` の書込みの組を競合と報告して止まることがあった。
製品の競合ではない。

macOS 27 の `libswift_Concurrency` を `dyld_info -disassemble` で読むと、順序の根拠は次のとおりだった。

- `swift_continuation_throwingResumeImpl` は `casal` で状態を更新し、TSan のフックも呼ぶ。
- `swift_continuation_awaitImpl` は再開済みなら、`ldapr` で読んだ後にフックを呼ばず続きへ進む。

順序はハードウェアで保証されるが、再開が `await` より先だと TSan には acquire が見えない。
送信側のスレッドが遅れるほど、この順になりやすい。

タスクが次に実行器を変えた後は、報告されなくなる。
結果を main actor へ返してから読む経路に出ず、`await` の直後に同じ関数で読む `Document.audio` にだけ出たのはこのためである。

## 再現

全コアに 10 個の CPU 消費プロセスを動かした TSan の Swift 全体で測った。

| ビルド | 実行回数 | `Document.swift:59` の報告が出た回数 |
| --- | --- | --- |
| 注釈なし（`01e09aa`） | 3 | 3 |
| 注釈あり | 14 | 0 |

`DevicePreparationTests` の調査でも、注釈なしの同じ報告が負荷下で 6 回中 6 回、負荷なしで 49 回中 1 回出ている。
これまで再現しにくかったのは、送信側の遅れが足りなかったためである。
注釈なしの全報告はこの 1 種類だった。
スパイクの `c-probe` は注釈の有無によらず負荷下の 5 回で報告がなかったが、`await` の直後に読むので同じ処置を入れた。

待ちを使わない決定的な再現として、`spikes/bridge/sanitizers/Continuation.swift` は再開が必ず先になる順序を作る。
相手のスレッドを待つ時に TSan へ順序を与えないよう、緩和された原子変数で回り、`swift_once` を通る遅延初期化のグローバルを避け、解放もしない。
実際に、遅延初期化のグローバルを使った最初の版は報告が出ず、main actor へ戻ってから読む版でも出なかった。

## 決定

- **呼出しごとの注釈**: `ContinuationHandoff` が、`resume` の前に `__tsan_release`、`await` の後に `__tsan_acquire` を同じアドレスで呼ぶ。
  アドレスは呼出しごとのオブジェクトなので、別の呼出しの間に順序を作らない。
- **検査ビルドだけ**: `check-bridge-tsan` が `-D ECHODE_TSAN` を渡す。Swift にはサニタイザーを判定する条件がない。
  それ以外のビルドでは空の値型と空の関数になり、確保も呼出しも残らない。
- **1 つの入口**: 文書の完了は `awaitCompletion` と `completeCommand` の組だけを通し、成功と拒否の両方で同じ注釈を通る。
  拒否の側は、`await` の後の読取りが計装のないランタイムの中で行われ、注釈なしでも報告されなかった。
  将来の読取りにも効くよう、`Result` を作ってから `release` する。
- **宣言**: `__tsan_*` は `<sanitizer/tsan_interface.h>` の公開の関数で、検査ビルドだけ `@_silgen_name` で宣言する。
  遅延初期化を挟む `dlsym` は、`swift_once` が余分な順序を作るので使わない。

この注釈は、[ADR 0121](0121-startup-recovery-and-errno-validation.md) が避けた偽の同期ではない。
ハードウェアにある順序を、再開と `await` の同じ組に限って TSan へ伝えるだけである。

## 比較した案

| 案 | 判定 |
| --- | --- |
| suppression | 同じ箇所の本物の競合も隠れる。方針どおり使わない |
| `await` の後に `Task.yield()` | 製品の全呼出しに切替えの遅延が残り、ランタイムの内部動作にも依存する |
| `Snapshot` に原子変数やロック | 全ビルドに検査専用の負担が残り、不要なロックで隠すことになる |
| 文書ワーカーで復号してから再開 | ワーカーを JSON 復号で待たせ、他の読取りに効かない |
| 継続で生ハンドルを渡し `await` 側で `Snapshot` を作る | Swift の書込みは消えるが、Rust 側の本体の読取りが同じ形で残る |
| `swift_continuation_await` の差替え | 全継続に効くが、非公開の配置に依存し、再開との隙間も閉じない |
| 全呼出しで共通のアドレスに注釈 | 無関係な呼出しの間に順序が増え、本物の競合を隠す |

## 検出能力の確認

`check-bridge-tsan` の対照 `continuation-*` は、製品の `ContinuationHandoff.swift` をそのままビルドする。

- `continuation-unannotated`: 注釈なしでは `Value.init` の書込みとの競合を報告する。ランタイムの穴がまだあることの確認である。
- `continuation-annotated`: 注釈があれば報告しない。
- `continuation-late-write`: 注釈があっても、再開の後の書込みは `writeAfterResume` の競合として報告する。注釈が順序を増やしすぎないことの確認である。

`DocumentCompletionOrderTests` は、実際の文書ワーカーと `completeCommand` で再開を先にして、スナップショットを `await` の直後に読む。
`-D ECHODE_TSAN` を外した TSan ビルドでは、この回帰テストが `Snapshot.trackCount` で競合を報告することを確かめた。
拒否の側の同じテストは注釈なしでも合格し、回帰を見分けられなかったので残していない。

release ビルドの `Document.o` には `__tsan_*` も `ContinuationHandoff` の関数の呼出しも残らず、型の反射情報だけが残ることを確かめた。

`check-tsan-runtime` と、Rust・Swift の既存の陽性対照は変更していない。

## 残る制約

- `swift test --sanitize thread` を `-D ECHODE_TSAN` なしで直接使うと、この誤検出は残る。正式な入口は `make check-bridge-tsan` である。
- 将来の Swift ランタイムが穴を塞ぐと `continuation-unannotated` が失敗する。その時は注釈を外せるかを調べる。
- `await` の直後に読まない継続（`RecoveryConsent` の main actor での応答など）には入れていない。
- 負荷下の TSan 全体では、この報告とは別に 5 秒のポーリングの期限切れが出ることがある。注釈ありの 14 回では `DevicePreparationTests` が 3 回、`MixerStepTests` が 1 回だった。
  `MixerStepTests` 単独は同じ負荷の 4 回とも合格した。いずれも TSan の報告はなく、この注釈では解決しない。
  （2026-10-31 追記: この 14 回は全面走査を主アクターの外へ移す修正の前である。修正後の数値は[デバッグ手順](../debugging.md)の `Status did not arrive` を参照。）
- 作業中の `make verify` で、実 HAL の `write_planes` と自動保存の zlib の組も 1 回報告された。これは別の原因で、`FolderDisclosureTests` を機器なしへ分離した（[デバッグ手順](../debugging.md) の TSan の項目 1）。
