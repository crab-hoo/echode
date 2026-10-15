# 同じデスクトップでの Computer Use 再監査

日付: 2026-10-14。対象は `534ca6467f425475f5f5c7c83aa96af316e51522` と、その上の ADR 0091・0092 の未コミットの修正。製品アプリを作り直し、Computer Use で終了・起動・パネル・座標・ドラッグ・キーボードを操作した。製品と正式な検査のソースは変更していない。[検査資料](../../target/verification/computer-use-same-desktop-20261014/evidence.json)。

**製品の操作経路では新しい不具合を確認しなかった。CU-01 の修正を再確認し、CU-02 の通常検査からの分離も確認した。ただし、正式な `spike-playhead` はまだ通過しない。起動方式に関する P2 / CU-03 を残す。診断用コピーの合格を、正式な検査の合格へ置き換えない。**

## 製品の実画面

前回の一時プロジェクトをコピーした `/private/tmp/echode-same-desktop-20261014-ef8urybw/DesktopAudit.echode` を使用した。既存のユーザープロジェクトには編集を加えていない。検査時の出力表示は MacBook Air のスピーカー。出力の選択やシステムの表示設定は変更していない。

- **停止中の位置移動**: 0→2拍で、数値とアレンジ・ピアノロールの縦線が2拍へ移動。
- **再生と一時停止**: 再生画像の数値3.00拍と両画面の縦線が約3拍へ進み、一時停止の3.41拍で両方もその位置に止まった。編集による描画の起動は不要だった。
- **ノートのドラッグ**: C♯4・開始1拍・長さ6拍を、D4・開始2拍・長さ6拍へ移動。左端を1拍右へ動かして D4・開始3拍・長さ5拍になった。Undo で2／6、さらに Undo で元の C♯4／1／6へ戻った。数値欄、インスペクター、ノートの描画と同じIDを照合した。
- **フォルダと音声取り込み**: 新しい一時フォルダを選択し、トラック4を追加。`test-tone.wav` のダブルクリックで、2拍の位置に2秒の音声を取り込めた。前半の小さい振幅と後半の大きい振幅が順番どおり表示された。Undo で消え、Redo で同じクリップIDが戻った。
- **履歴復帰**: Redo 後の現在の版 `2b88887db17d` から、Undo 後の `95f83e2a0ac6` の「戻す」を画像の座標で押した。新しい版 `d7eb95989ec4` が増え、トラック4の音声が消えた。復帰の Undo で同じ音声が戻った。
- **ペインと文書交換**: ノートエディタを閉じて再表示できた。トリム済みデータの別文書 `WaveTrim.echode` を開き、元音声の後半だけを示す2〜4拍の波形を確認。新しい文書の0→3拍の位置移動も両画面に反映された。音声トリムのUI操作を検査した主張ではない。
- **保存・終了・再起動**: 元の文書へ戻し、通常終了して再起動。4番目のトラックまでスクロールし、音声クリップID `01a13aa3-bc8e-7031-9f2e-e7e8ac887326` と波形が残ることを確認した。元のノートと Vital も残った。再起動後の0→2拍も通過した。強制終了は今回再実行していない。

画像は [位置移動](../../target/verification/computer-use-same-desktop-20261014/locate-two.png)、[再生](../../target/verification/computer-use-same-desktop-20261014/playing.png)、[一時停止](../../target/verification/computer-use-same-desktop-20261014/paused.png)、[ドラッグ](../../target/verification/computer-use-same-desktop-20261014/note-moved.png)、[左端の伸縮](../../target/verification/computer-use-same-desktop-20261014/left-edge-resized.png)、[音声取り込み](../../target/verification/computer-use-same-desktop-20261014/import-track-four.png)、[履歴復帰](../../target/verification/computer-use-same-desktop-20261014/history-restored.png)、[復帰のUndo](../../target/verification/computer-use-same-desktop-20261014/history-undo.png)、[トリム済み波形](../../target/verification/computer-use-same-desktop-20261014/trimmed-wave.png)、[再起動](../../target/verification/computer-use-same-desktop-20261014/after-restart.png) にある。

上記の Echode の操作では、前回の `noWindowsAvailable`、画面取得の `-3811`、取得サービスの切断は再発しなかった。パネルを開いた直後に次のショートカットを続けて送ると、パネルの準備前に入力されることがあった。新しい画面を取得してから操作を続け、結果を確認した。

## CU-02 の修正確認

実ウインドウを使う4件は `ECHODE_SPIKE_PLAYHEAD=1` のときだけ有効になった。通常の世代・操作結果のテストは残り、Makefile、`scripts/verification.json`、選択のテスト、ADR 0092 も更新されている。遮蔽時に主要な期待値を省略して合格する分岐は削除され、条件不足を不合格として記録するようになった。この区分の指摘は対応済みとする。

## CU-03 / P2: 正式な実機回帰の起動方式が表示条件を満たせない

### 再現と切り分け

正式な入口は `Makefile:29` の `swift test --filter PlayheadTests`。製品アプリが終了した状態で実行しても、実ウインドウの4件すべてが `requireVisible` で止まった。画面1枚、点灯、`visible=true`、`onActiveSpace=true`、`miniaturized=false` だが、`occluded=true` が続く。主要な期待値は未検証、終了コードは非ゼロである。単に「対象を同じデスクトップへ置く」だけでは、この入口の失敗は解消しなかった。

実行中の30秒間、公開APIでウインドウの位置と表示状態を読み取った。所有者は `swiftpm-testing-helper`。検査の `Echode playhead` ウインドウは、約900×682ポイントで現在の画面の一覧にも存在した。[ウインドウ記録](../../target/verification/computer-use-same-desktop-20261014/window-watch.json)。別 Space にある製品本体へ座標を送った前回の問題とは、条件が異なる。

一時コピーで、次の2つを比較した。製品の描画コードの Swift ファイルは、リポジトリとコピーですべて同じ識別値だった。期待値を削除したり、`beginMeasurement()` を有効にしたり、期限を延ばしたりはしていない。

1. Swift Testing の起動を維持し、`finishLaunching()` を一度呼ぶだけのコピー: 同じ遮蔽判定で失敗した。[結果](../../target/verification/computer-use-same-desktop-20261014/launch-diagnostic.json)。
2. 元の4件の期待値の本体をそのまま、独立した AppKit の実行ファイルから順に呼び、`NSApplication.run()` でイベントループを動かすコピー: **4件とも完了、終了コード0**。4回の表示条件の確認もすべて `visible=true`・現在の Space・非遮蔽だった。[ログ](../../target/verification/computer-use-same-desktop-20261014/runloop-diagnostic.log)。

これにより、現在の失敗を描画実装の不良としては認定しない。起動方式の比較が失敗／成功を分けた。Swift Testing のヘルパーと AppKit のイベント処理のどの内部動作が遮蔽状態を更新しないのかまでは、この比較だけで特定していない。正式な入口は現状のままなので、その合格にはしない。

### 実行役への具体的な修正指示

1. `spike-playhead` の実ウインドウ部分を、AppKit のイベントループを持つプローブで起動する。既存の `EchodeAppProbe` のように、メインスレッドでアプリを初期化し、`Task { @MainActor ... }` へ検査を載せ、`NSApplication.run()` を動かす。単に `finishLaunching()` を足す修正では、今回の再現は解消しなかった。
2. 4件の検査ロジックは共通の検査用処理へまとめ、診断用コピーを製品の別実装として残さない。0→2拍、停止中の休止、同じ feed で再描画しないこと、再生継続、一時停止、破棄、開き直し、計画交換、文書交換の全期待値を保持する。
3. 現在の Space・非遮蔽を確認するガードは維持する。条件不足は未検証として非ゼロで終える。通常の `make check` からは実ウインドウ部分を引き続き除く。
4. 合格や失敗のウインドウ状態は、`close()` の**前**に保存する。現在の `exercise` は閉じてから状態を採り、診断用コピーの合格JSONは `status=passed` なのに `visible=false` になった。検査中の条件と後片付け後の状態を混同させない。
5. Makefile と検査定義・選択のテストを正式な入口に合わせ、`make spike-playhead` を再実行する。4件の期待値が走ったログ、実行時の表示条件、JSON、終了コード0を一組で確認する。今回の診断用コピーの合格を流用しない。

診断用の一時ソースの場所と比較の識別値は [検査資料](../../target/verification/computer-use-same-desktop-20261014/evidence.json) に記録した。

## 検査と制約

着手時に `make verify-plan` を実行した。最終の検査結果と現在のソース識別値は [検査記録](../../target/verification/report.md) を参照する。正式な `spike-playhead` の失敗は残し、見送りへ変更しない。

`spike-draw`（大規模描画）、`spike-engine`（10分間の音声負荷）、`spike-bridge`（橋渡しの性能）は今回の操作再監査では未実施として理由を記録する。ピンチ、感圧、触覚、聴感の一致、機器の抜き差し、CLAPのパラメーター・状態の往復も合格に含めない。

検査用ウインドウの切り分けで Terminal の画面操作も試したが、Computer Use の安全制限で拒否されたため操作していない。許可された既存の検査の実行経路と、一時コピーでの比較で調査を続けた。独立した診断プログラムの初回起動では Xcode の Testing フレームワークの実行時パスが不足した。確認したフレームワークのパスを指定して起動できた。この起動エラーを製品の不具合には数えない。

## 2026-10-15 の対応

CU-03 の起動方式を AppKit の起動完了とイベントループへ変更し、4件が正式な入口から通過した。ウィンドウを閉じる前の条件を保存するようにした。最新の判定は `target/verification/report.md` を参照する。CU-04〜CU-06 の利用経路の回帰検査と、誤りを戻したコピーを拒否する自動検査も追加した（[ADR 0093](../decisions/0093-product-contract-gate.md)）。プラグインGUIと、他アプリからの復帰を含む入力経路全体の実機検査を合格とする変更ではない。
