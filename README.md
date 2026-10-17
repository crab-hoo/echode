# Echode

macOS 専用のオープンソース DAW。Phase 1 を完了し、Phase 2 を開始しました。[現在の範囲と制約](docs/status.md)、[Phase 2 の進め方](docs/phase2.md)、[資料の入口](docs/README.md)、[計画](docs/PLAN.md)、[設計](docs/ARCHITECTURE.md) を参照してください。

ライセンスは GPL-3.0-or-later。全文は [LICENSE](LICENSE) にあります。

## 開発を始める

M4 ファミリー以降の Apple Silicon、macOS 27 以降、Xcode 27 と Rust を使います。Rust は `rust-toolchain.toml` に固定しています。CPU の対象方針は [ADR 0112](docs/decisions/0112-m4-minimum.md) を参照してください。

```sh
make doctor           # 環境、アプリと検査結果の古さを確認
make setup            # 通常検査の追加ツールを初回に用意
make setup-bridge     # 橋渡しの動的検査用ツールを初回に用意
make app              # target/Echode.app を作る
make verify-plan      # 変更に応じた検査を確認
make verify           # 自動検査と実機検査の記録を確認
make debug-report     # 既存の検査ログと診断資料を収集
```

エージェントは [AGENTS.md](AGENTS.md) と [開発手順](docs/development.md) を読んで作業してください。検査の実行・見送りと有効期限は [検査手順](docs/verification.md)、失敗の調査は [デバッグ手順](docs/debugging.md) にまとめています。`make check` は通常検査と必須の製品回帰を実行し、追加検査の選択は `make verify` が行います。`just` が入っていれば、定義された同名レシピも使えます。

`make app` は、ログインキーチェーンに `Echode Local` があればその証明書で、なければ ad-hoc で署名します。証明書は `make sign-setup` で作成できます。配布は `make dist`、XcodeGen のプロジェクト生成は `make xcode`。Xcode のライセンスに未同意なら、利用者がターミナルで `sudo xcodebuild -license` を確認してください。

## 開発用アプリの操作

`target/Echode.app` を開き、`.echode` パッケージを作成します。トラックを追加して、下部のピアノロールで打ち込めます。空白のダブルクリックで挿入、ドラッグで移動、両端のドラッグで長さを変更します。選択したノートは数値入力と「変更を適用」でも編集できます。

Tab / Shift-Tab は次 / 前のノート、上下は半音（Option で1オクターブ）、左右は16分音符の移動、Shift-左右は長さ変更、Return は挿入、Delete は削除、Escape はドラッグの取消です。鍵盤の横にはディグリー（I〜VII）を表示します。表示する調と長短調は画面内の設定で、プロジェクトの調を変更しません。

変更は Rust のアクションとジャーナルへ記録し、プロジェクトの履歴へ自動コミットします。Undo / Redo は Command-Z / Shift-Command-Z。左右と下部のペインは Option-Command-1 / 2 / 3、コマンドパレットは Shift-Command-P で開閉します。アレンジはスクロール、ピンチで時間、Option-ピンチで高さ、Shift-ピンチで両軸を拡大します。

Space は再生／一時停止、Command-. は停止です。停止と一時停止は位置を保ちます。拍を指定する「移動」で位置を変更できます。「音声出力」はシステムの出力への追従か特定の機器を選び、現在のサンプルレートとバッファ長を表示します。

「プラグイン」で CLAP 一覧を開き、ダブルクリックで選択トラックへ挿入します。Enter ならブラウザを開いたまま挿入します。楽器は内蔵 Sine Pad を置き換え、エフェクトは入力音を処理します。プラグイン GUI はまだ開けません。履歴から以前の版へ戻せ、音声ファイルは選択トラックへ取り込めます。ループとメトロノームは再生設定です。

## MCP でヘッドレスのセッションへ接続する

既存の `.echode` パッケージを指定して、別のターミナルで `target/release/echode serve PACKAGE SOCKET` を起動します。実行ファイルは `cargo build --release -p echode-cli` で作れます。MCP クライアントには、その実行ファイルの絶対パスを command、`["mcp", "SOCKETの絶対パス"]` を args として指定します。

同じビルドの serve と mcp を使います。MCP は登録済みアクションのツール、まとめた編集、Undo / Redo、概要とトラックの読み取りを提供します。編集は既存のジャーナルへ記録されます。起動中の GUI アプリへの接続と再生・機器操作は初版の対象外です。GUI で開いている同じパッケージを serve で開き直さないでください。接続範囲と検査は [ADR 0115](docs/decisions/0115-phase2-mcp.md) を参照してください。
