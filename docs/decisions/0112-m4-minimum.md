# ADR 0112: 対象チップを M4 ファミリー以降にする

- 日付: 2026-10-17
- 状態: 採用（M4 向けビルド条件は ADR 0113 で接続。機種判定と SME は未実装）
- 範囲: macOS 27 以降という OS 要件は維持し、チップの最低世代を M4 にする。ADR 0110 の M1〜M3 も対象とする前提と、最適化監査の M1 基準を置き換える

## 背景と決定

macOS 27 の最適化監査後、ユーザーから「チップは M4 以降前提で構わない」と指定された。対象を **macOS 27.0 以降、M4 ファミリー以降の Apple Silicon（arm64）** とする。M4 の標準・Pro・Max などを含み、M1〜M3、Intel、Rosetta への対応は求めない。

これにより、M1 で動くことを理由に CPU 最適化を制限する必要はなくなる。ただし、チップ世代を絞っただけで、GPU の待ち、メモリ確保、ロックなどのリアルタイム制約が緩むわけではない。

## 最適化への影響

- **コンパイラー**: 固定した M4 を比較基準にする。Rust は `-C target-cpu=apple-m4`、C / C++ は対応する M4 ターゲットを比較候補にする。開発機が将来 M5 以降になっても配布の基準を変えないため、`native` を配布方針にしない。依存ライブラリと Swift を含む設定の統一、macOS 27 の最低 OS の修正、リリース性能と動的検査は実装時に行う。
- **SME / SME2**: M1〜M3 が対象に含まれることを理由に除外しない。まず非 RT の解析、バッチ処理、オフラインの重い DSP で、公開 API の Accelerate、SME を利用する既存実装、CPU SIMD、Metal を同じ入力と意味で比較する。自作のアセンブリーを先に追加しない。小さいミックスのブロックで有利かは別に測り、ストリーミングモードの ABI・レジスター状態・呼出し境界を守る。M4 対象を ARMv9.2 全機能や通常モードの SVE が使えることと同一視しない。
- **GPU**: M4 は Apple9 ファミリーを基準にする。Metal 4 の再利用可能なコマンドと allocator は引き続き比較候補。M5 の Apple10 以降だけの能力や新しい演算機構を、M4 でも使えると仮定しない。世代固有の経路は公開の能力問い合わせで選ぶ。
- **画面とメモリ**: 120 Hz、搭載メモリ量、メモリ帯域、CPU のコア数は M4 の標準・Pro・Max と外付け画面で異なる。固定値にせず、画面の情報と Workgroup の推奨並列数などを使う。
- **既存の優先順**: Workgroup の製品接続、最低 OS の統一、PCM の共有、静的な描画準備の再利用を先に進める。SME の採用だけで、接続漏れやコピーが解消するわけではない。

## 確認した根拠と限界

- 手元の Rust 1.96.1 の `rustc --print target-cpus` に `apple-m4` があり、`rustc -C target-cpu=apple-m4 --print cfg` が成功した。M4 ターゲットを指定できる確認であり、製品の設定変更や速度の実測ではない。
- Xcode 27.1 の `xcrun clang -mcpu=apple-m4 -dM -E -x c /dev/null` は `__ARM_FEATURE_SME` と `__ARM_FEATURE_SME2` を定義する。M4 ターゲットのコンパイラー機能の確認であり、SME 命令を製品で実行して検査した意味ではない。
- [Apple Silicon CPU Optimization Guide](https://developer.apple.com/documentation/apple-silicon/cpu-optimization-guide)は SIMD と SME の最適化を扱う。M4 の SME レジスター対応は [LLVM の実装](https://github.com/llvm/llvm-project/pull/119171)でも確認できる。
- [Apple の GPU 対応表](https://developer.apple.com/metal/capabilities/)は M4 を Apple9、M5 を Apple10 に分類している。最低 OS の版だけで GPU の能力を決めない。

今回変えたのは対象方針と資料である。CPU 世代の起動時判定、固定 M4 向けビルド、SME カーネル、追加の性能計測、実機・聴感検査は行わない。M4 より前の実機を用意した検査や起動拒否を実装済みとは扱わない。

## 基盤の保守

README、PLAN、ARCHITECTURE、status、最適化監査とその Canvas に新しい前提を反映する。過去の監査と ADR の測定値は保持し、前提変更を追記する。対象設定や起動判定の実装時は、製品・依存のビルド条件、診断、配布の契約、検査機のチップとログを揃える。今回は実行入口・検査条件を変えないため、Makefile と `scripts/verification.json` に新しい検査は追加しない。最後に `make verify` を実行し、変更されていない製品検査は有効な結果を再利用する。

## 後日対応（2026-10-17）

[ADR 0113](0113-product-macos-workgroup-and-caches.md)で、固定 M4 向けの Rust・C 依存、製品・橋渡し検査の最低 OS 27.0、組立前の Mach-O 検査を接続した。Swift の明示的な M4 CPU 指定は SDK の機能条件の不一致でコンパイルできず、SDK 互換の arm64 を維持する。上の「今回未実装」はこの ADR 作成時点の記録である。SME とチップ名による起動判定、実機性能の改善量は未確認のままである。

[ADR 0114](0114-m4-build-baseline.md) で、Swift と Clang の依存モジュールへ M4 指定を対で渡して不一致を解消し、全ての自前のビルド対象へ基準を揃えた。汎用設定が測定で速い場合の例外も方針へ追加した。
