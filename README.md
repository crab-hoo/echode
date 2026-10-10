# Echode

macOS 専用のオープンソース DAW。計画は [docs/PLAN.md](docs/PLAN.md)、技術設計は [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

ライセンスは GPL-3.0-or-later（GNU General Public License の版 3、または利用者が選ぶそれ以降の版）です。全文は [LICENSE](LICENSE) にあります。

## 開発

対象は Apple Silicon の macOS 27 以降です。Rust の stable と Xcode 27 を使います。

```sh
make verify-plan      # 変更箇所から必要な検査と計測を選ぶ
make verify           # 通常・追加検査を実行し、計測の未実行理由も確認する
make check            # フォーマット、clippy、Rust と Swift のテスト、ライセンス
make spike-isolation  # プラグイン隔離の往復時間を release ビルドで測る
make spike-draw       # 1,000 トラック・10 万ノートの描画がリフレッシュに追従するか測る
make spike-engine     # 1,000 ノードを 64 フレームで 10 分間処理する
make spike-history    # 1,000 トラックの保存と、過去の版の読み込みを測る
make spike-input      # ピンチ、触覚、感圧の確認ウィンドウを開く
make check-input      # 入力の Swift 単体テスト（ウィンドウなし）
make spike-render     # WAV の一括書き出しを release で計測する
make check-bridge     # Rust / Swift の橋渡しを Swift 6 の厳密チェックで確かめる
make spike-bridge     # 同じ橋渡しの検証に、スナップショットと配列の計測を加える
make setup-bridge     # 検査用の固定版 nightly と Miri の標準ライブラリを初回だけ用意する
make check-bridge-asan # Rust の ABI と Swift の往復を別プロセスでメモリ検査
make check-bridge-tsan # 同じ橋渡しのデータ競合の検査（ASan とは別に実行する）
make check-bridge-miri # Rust の所有権の検査を 3 つの seed で実行する
```

`just` が入っていれば `just check` でも同じことができます。

変更を終えたら `make verify` を実行します。[検査の手順](docs/verification.md) に実行時期と記録の方法があります。対応は `scripts/verification.json` にまとめ、終了フックと CI も同じ入口を使います。計測や実機が必要な項目は `python3 scripts/verify.py --run ID` で実行するか、`--defer ID --reason '理由'` で未実行を記録します。結果は `target/verification/report.md` に残り、ソースやツールが変わると古い記録は無効になります。

橋渡しを変更したら ASan と TSan、Rust のポインタや所有権を変更したら Miri も実行します。製品用の Rust は `rust-toolchain.toml` のままで、動的検査だけが `scripts/bridge-toolchain.env` の nightly を使います。CI は Xcode 27 の環境で Swift の検査と、別ジョブの ASan、TSan、Miri を実行します。これらの検査には音声デバイスや描画ウィンドウは要りません。

ASan は Rust と Xcode のバージョン確認の ABI が異なるため、確認を無効にせず、Rust 側の検査と Swift 側の往復を分けています。TSan は両側を同じプロセスで検査します。ASan の検査範囲と製品に引き継ぐ条件は [ADR 0044](docs/decisions/0044-swift-bridge.md) にあります。

Xcode のライセンスに未同意だと、Swift と Metal を使う検証はビルドできません。ターミナルで `sudo xcodebuild -license` を実行してください。
