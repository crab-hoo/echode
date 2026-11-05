# ADR 0114: 自分たちでビルドするコードの M4 基準を統一する

- 日付: 2026-10-17
- 状態: 採用
- 範囲: Rust、C / C++、Swift、Xcode の入口、単体テスト、橋渡し・描画・入力の補助プログラム。ADR 0113 の Swift の M4 指定見送りを置き換える

## 方針

ユーザーから「全ての環境をできれば M4 以降向けにしたい。そうしない方がパフォーマンスが上がるならその限りではない」と指定された。自分たちでビルドするコードは、macOS 27 以降・固定した `apple-m4` を共通基準にする。開発機に追従する `native` は使わない。汎用設定が同じ処理で速いと測定できる場合は、根拠と条件を残して例外を認める。指定しただけで速度改善としない。

Apple が配布するシステムライブラリ・SDK のバイナリ、コンパイラー自体、外部プラグインの既存バイナリまで再ビルドする方針ではない。GPU のソースは従来どおり使用中の Metal デバイスでコンパイルする。M4 の Apple9 を最低世代とし、M5 固有機能や画面レートなどを一律に仮定しない。

## Swift の不一致の切り分け

以前の失敗は SDK 自体の不具合と確定していなかった。独立した最小パッケージと新しいキャッシュでも、Swift の `-target-cpu apple-m4` だけでは Testing の interface と SwiftShims PCM の CPU 機能が一致しなかった。

Swift の指定に `-Xcc -mcpu=apple-m4` を併せ、Clang の依存モジュールにも同じ基準を渡すと、新しいキャッシュでコンパイルと Swift Testing の実テストが通った。C の検査用関数は M4 の SME2 定義を確認し、Swift の生成 LLVM IR も `target-cpu=apple-m4` だった。Package.swift の各ターゲットに指定する方法と直接の swiftc の両方を確認した。SDK の更新待ちとはせず、モジュール検証・explicit module build は有効なままにする。

観測した不一致は設定の渡し方で解消した。Swift コンパイラー内部の原因箇所まで確定したという意味ではない。失敗は `target/m4-build-audit/swift-only.log`、対の指定は `paired.log`、Package.swift の指定は `manifest.log`、IR は `cpu.ll`。以前の失敗ログも保持する。`-target-cpu` の意味は [Swift driver のオプション定義](https://github.com/swiftlang/swift-driver/blob/main/Sources/SwiftOptions/Options.swift) も参照する。

## 接続する入口

- Cargo の Rust と C / C++ 依存は既存の `.cargo/config.toml` を使う。
- 製品の2つの Swift Package と、描画・入力の2つのスパイクは、全 Swift ターゲットとテストへ上記の対を指定する。C / C++ ターゲットにも M4 を指定し、スパイクに残っていた最低 OS 14 を27へ揃える。
- 直接の swiftc と rustc は `scripts/build-platform.env` を共通の入口にする。ASan / TSan の正しさを確かめる検査用プログラムも同じ CPU を使う。配布・隔離用の小さい C の fixture も揃える。古い最低 OS を拒否する負の検査だけは意図的に古い OS を指定する。
- `project.yml` の Xcode の Swift / C / C++ と architecture を揃える。生成されたプロジェクトを手で編集しない。
- ローカルの通常検査と CI のネイティブ検査は、実行前に報告されたチップ・OS・architecture が M4 以降 / macOS 27以降 / arm64 か調べる。CI のラベルだけでチップを保証したとしない。選択や advisory のメタデータだけを処理するジョブには、M4 バイナリの実行がない。リモート CI の実行・機種の確認は今回のローカル合格と区別する。製品アプリの起動時判定を追加した意味ではない。
  2026-11-05、GitHub に M4 のランナーがないため、CI はネイティブ検査をせず静的検査だけにした（[ADR 0269](0269-github-public-ci-scope.md)）。

## 性能の比較

Apple M4 / macOS 27.0 / Swift 6.4 / `-O`。既存の C ABI 橋渡しプローブを同じ M4 向け Rust ライブラリへリンクし、Swift を汎用 arm64 と M4 の2種類で作った。実行順を交互にして各12回比較した。

- 1,000回のスナップショット取得と借用読み取りの中央値: 汎用49.21 μs、M4 48.25 μs。
- 100,000ノートの配列を借りて100回走査する中央値: 汎用140.95 ms、M4 140.67 ms。

この測定では同程度で、M4 指定による性能悪化の根拠は得られなかったため、共通基準として採用する。小さい差を有意な高速化とせず、アプリ全体・実機の音声負荷・M4 Pro / Max などへ一般化しない。生データと全12回の出力は `target/m4-build-audit/bench/comparison.json`、コンパイルログも同じディレクトリに保持する。

## 検査と保守

`check-verification` が既に実行する `scripts/tests` の回帰へ、4パッケージの全ターゲット、Swift / C / C++ / 直接の Rust の実際の IR、新しいキャッシュでの C import と Swift Testing、Xcode の設定、実行ホストの判定を追加する。共有 env の変更は全検査の入力を失効させる。選択の検査も更新する。

以前の監査、ログ、ADR 0113 の判断は当時の記録として保持し、status・設計・開発・検査・デバッグ手順の現在の案内を更新する。最終合否は `make verify` と検査レポートを正とする。実 HAL で観測した未確定な TSan 警告は本件と別であり、機器なしの検査の成功を解決へ読み替えない。

最初の全検査では、通常回帰、製品契約27件と負の対照27件、RTSan、入力、全 feature、ASan が通ったあと、TSan が終了済みスレッドと新しいスレッドの errno アクセスを同じアドレスとして報告した。HAL の警告とは異なる。Rust 標準ライブラリだけの独立ライブラリに、呼出しだけの Swift をリンクし、同じ nightly・M4 / macOS 27・Apple TSan ランタイムで再現した。8親スレッドから各300回子を生成・joinし、子で metadata エラーと read_dir を8回行う。製品・外部 crate・オーディオ機器は含まない。製品の警告を一律に誤検出と扱わず、抑制や検査条件の緩和をせず同じ正式な入口を再実行する。内部の原因やこの警告の解決を確認した意味ではない。

証拠は `target/m4-build-audit/tsan-errno-product-before.log`、`tsan-errno-verification-before.log`、`verification-before-errno.json`、独立再現は `errno-probe/src/lib.rs`・`errno-probe/Main.swift`・`errno-standalone.log`。以前のデバッグ資料の同型の警告に、現在の環境の証拠を追加する。

同じ正式な入口の再実行では、自動検査9項目、画面なしの `spike-arrange-frame` / `spike-piano-frame`、`spike-bridge` が合格した。基盤89テストと製品契約27件・負の対照27件を含む。TSan の警告の解決やリモート CI の成功とは扱わない。配布アプリの組立・起動、実機・聴感検査は行っていない。選択された実機項目はユーザー担当として見送り、`spike-render` の時間計測と依存版追加のない `advisories` も理由付きで未実行を記録した。最新の再利用・見送りの有効性は検査レポートを参照する。

2026-10-19 追記: errno 警告は C のみでも再現し、スレッド寿命の追跡不足を修正した検査専用ランタイムへ移行する。
当時の失敗・再実行の記録は保持する。原因・対照・現在の入口は [ADR 0121](0121-startup-recovery-and-errno-validation.md) を参照する。
