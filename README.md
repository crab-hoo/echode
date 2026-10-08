# Echode

macOS 専用のオープンソース DAW。計画は [docs/PLAN.md](docs/PLAN.md)、技術設計は [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

ライセンスは GPL-3.0-or-later（GNU General Public License の版 3、または利用者が選ぶそれ以降の版）です。全文は [LICENSE](LICENSE) にあります。

## 開発

対象は Apple Silicon の macOS 27 以降です。Rust の stable と Xcode 27 を使います。

```sh
make check            # フォーマット、clippy、テスト
make spike-isolation  # プラグイン隔離の往復時間を release ビルドで測る
make spike-draw       # 1,000 トラック・10 万ノートの描画がリフレッシュに追従するか測る
make spike-engine     # 1,000 ノードを 64 フレームで 10 分間処理する
make spike-history    # 1,000 トラックの保存と、過去の版の読み込みを測る
make spike-input      # ピンチ、触覚、感圧の確認ウィンドウを開く
```

`just` が入っていれば `just check` でも同じことができます。

Xcode のライセンスに未同意だと、Swift と Metal を使う検証はビルドできません。ターミナルで `sudo xcodebuild -license` を実行してください。
