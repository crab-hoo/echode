# Echode

macOS 専用のオープンソース DAW。計画は [docs/PLAN.md](docs/PLAN.md)、技術設計は [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

ライセンスは GPL-3.0-or-later（GNU General Public License の版 3、または利用者が選ぶそれ以降の版）です。全文は [LICENSE](LICENSE) にあります。

## 開発

対象は Apple Silicon の macOS 27 以降です。Rust の stable と Xcode 27 を使います。

```sh
make app              # 開発用の target/Echode.app を作る
make spike-app        # 製品の描画スレッドと GPU バッファを実機で確認する
make verify-plan      # 変更箇所から必要な検査と計測を選ぶ
make verify           # 通常・追加検査を実行し、計測の未実行理由も確認する
make check            # フォーマット、clippy、Rust と Swift のテスト、ライセンス
make spike-isolation  # プラグイン隔離の往復時間を release ビルドで測る
make spike-draw       # 1,000 トラック・10 万ノートの描画がリフレッシュに追従するか測る
make spike-engine     # 1,000 ノードを 64 フレームで 10 分間処理する
make spike-audio      # 製品の CoreAudio 接続と停止・再起動を無音で確認する（画面なし）
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

`make app` の後で `target/Echode.app` を開くと、3 ペインの開発用アプリを使えます。
プロジェクトを `.echode` パッケージとして作成し、トラックを追加して、下部のピアノロールで打ち込めます。空白をダブルクリックして挿入し、ノートをドラッグして移動、両端をドラッグして長さを変えます。選択したノートは数値入力と「変更を適用」でも編集できます。
ピアノロールの Tab / Shift-Tab は次 / 前のノートへ移動し、画面外のノートも表示します。上下は半音（Option で 1 オクターブ）、左右は 16 分音符の移動、Shift-左右は長さ変更、Return はカーソル位置への挿入、Delete は削除、Escape はドラッグのキャンセルです。鍵盤の横に階名を表示し、表示する調と長短調を選べます。この設定はプロジェクトの調を変えません。
変更は既存の Rust のアクションとジャーナルへ記録し、自動コミットします。Undo / Redo はツールバーと Command-Z / Shift-Command-Z から使えます。
左右と下部のペインは Option-Command-1 / 2 / 3、コマンドパレットは Shift-Command-P で開閉します。
アレンジはスクロールで移動し、ピンチで時間、Option-ピンチでトラックの高さ、Shift-ピンチで両軸を拡大します。
上部の再生・一時停止・停止で内蔵 Sine Pad を鳴らせます。Space は再生／一時停止、Command-. は停止して先頭へ戻ります。拍を指定して「移動」で再生位置を変えられます。
「音声出力」で出力機器を選びます。既定はシステムの出力に追従し、特定の機器を選ぶと、その機器が抜けた時にシステムへ戻り、再接続で元の機器へ戻ります。サンプルレートとバッファのフレーム数は機器の現在値を表示します。
プラグインブラウザと履歴の画面、ループとメトロノームのアプリ操作、音声素材の取り込みは今後の接続です。

このバンドルはローカル開発用の ad-hoc 署名です。配布用の自己署名、XCFramework と生成する Xcode プロジェクトは、製品のパッケージングで用意します。

変更を終えたら `make verify` を実行します。[検査の手順](docs/verification.md) に実行時期と記録の方法があります。対応は `scripts/verification.json` にまとめ、終了フックと CI も同じ入口を使います。計測や実機が必要な項目は `python3 scripts/verify.py --run ID` で実行するか、`--defer ID --reason '理由'` で未実行を記録します。結果は `target/verification/report.md` に残り、ソースやツールが変わると古い記録は無効になります。

橋渡しを変更したら ASan と TSan、Rust のポインタや所有権を変更したら Miri も実行します。製品用の Rust は `rust-toolchain.toml` のままで、動的検査だけが `scripts/bridge-toolchain.env` の nightly を使います。CI は Xcode 27 の環境で Swift の検査と、別ジョブの ASan、TSan、Miri を実行します。これらの検査には音声デバイスや描画ウィンドウは要りません。

ASan は Rust と Xcode のバージョン確認の ABI が異なるため、確認を無効にせず、Rust 側の検査と Swift 側の往復を分けています。TSan は両側を同じプロセスで検査します。ASan の検査範囲と製品に引き継ぐ条件は [ADR 0044](docs/decisions/0044-swift-bridge.md) にあります。

Xcode のライセンスに未同意だと、Swift と Metal を使う検証はビルドできません。ターミナルで `sudo xcodebuild -license` を実行してください。
