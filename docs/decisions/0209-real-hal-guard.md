# 0209. UI テストのバンドルから実 HAL をなくし、触れたテストを名指しで止める

- 日付: 2026-11-01
- 状態: 採用
- 関連: [ADR 0136](0136-output-opening-off-worker.md)、[ADR 0207](0207-continuation-handoff-tsan.md)

## 背景と原因

UI テストは機器なしの応答差し替えを使う決まりだが、既定の `ProjectController()` は本物の `DeviceResult.send` を使う。
注入を忘れると実 HAL へ届く。2026-10-17、10-29、10-31 に繰り返し起きた。
見つかったのは、TSan が `write_planes` と自動保存の zlib の組を報告した時だけだった。

10-31 の例は `FolderDisclosureTests` の FX のドロップが開く窓の要求で、全体実行 13 回で出ず `make verify` で 1 回出た。
報告のスタックにテスト名も出ない。
実機テストはユーザーの担当なので、検査が機器を開くこと自体も避けたい。

## 実 HAL までの経路

HAL の関数を呼ぶのは Rust の `hardware.rs`、`stream.rs`、`audition.rs`、`workgroup.rs`、`echode-engine` の `device.rs` である。
`nm -u target/debug/libechode_ffi.a` で見ると、参照は次の 9 関数と、時計の変換の 2 関数だけだった。

- `AudioObjectGetPropertyData`、`AudioObjectGetPropertyDataSize`、`AudioObjectSetPropertyData`
- `AudioObjectAddPropertyListenerBlock`、`AudioObjectRemovePropertyListenerBlock`
- `AudioDeviceCreateIOProcID`、`AudioDeviceDestroyIOProcID`、`AudioDeviceStart`、`AudioDeviceStop`

Swift は CoreAudio も AVFoundation も import していない。

出力の登録までには段階があり、`AudioDeviceCreateIOProcID` は最後である。

1. `Session::refresh`（`play`、`prepare`、`output`、`refresh`、実際のプラグインを持つ文書のデバイス要求）か、試聴スレッドが `hardware::inventory()` を呼ぶ
2. `Session::begin` が機器を確認し、別のスレッドがヘルパーと素材を準備する
3. 準備の結果を受け取った作業スレッドが `AudioDeviceCreateIOProcID` を呼ぶ

3 だけを見張ると、文書が先に閉じた時や準備が失敗した時、出力機器がない時に素通りする。
これらの経路で必ず最初に来るのは 1 の `AudioObjectGetPropertyData` であり、機器の有無にも時間にもよらない。

## 比較した案

| 案 | 判定 |
| --- | --- |
| A. 検査の時だけ `DYLD_INSERT_LIBRARIES` で監視する | 製品への負担はないが、その検査の実行でしか働かず、ほかの実行は実機を開き続ける。`/usr/bin/swift` の転送が `DYLD_*` を落とし、arm64e のスライスも要る（`main_thread_stalls.py` と同じ手間） |
| B. `ProjectController` の既定の `sendDevice` / `sendAudio` をテストで使えなくする | テスト名は正確に出せる。だがテスト内の `ProjectController()` は 137 か所あり、大半は機器に触れない。コマンドの種類で判定すると、`device_control` の分岐を Swift へ写すことになる。テストが直接呼ぶ `document.audio` にも効かない |
| C. `opening.rs` の `HalOutputs` の前で検出する | 試聴は `HalOutputs` を通らない（`audition.rs` の `hardware::inventory()`）。製品のコードで環境変数を読むか、feature 違いのライブラリをもう 1 つ作る必要がある（ビルド約 39 秒、363 MB） |
| D. テストのバンドルにリンク時の代役を入れる | 採用。製品のコードは変わらない。経路も実行の種類も問わず、機器を一切開かない |

## 決定

- **代役**: `swift/EchodeApp/Tests/EchodeAppTests/RealHALGuard.swift` が 9 関数を `@_cdecl` で定義する。
  `libechode_ffi` は静的ライブラリなので、テストのバンドルをリンクする時に参照が代役へ解決される。
  代役は `kAudioHardwareIllegalOperationError` を返し、`Issue.record` と標準エラーへ報告する。
- **範囲**: `EchodeAppTests` のターゲットだけに入れる。通常、ASan、TSan、必須契約のどの実行にも同じように効く。
  製品、`EchodeAppProbe`、`EchodePlayheadProbe` はこのターゲットをリンクしないので `spike-*` は実物を使う。
  `EchodeBridgeTests` は別パッケージで、`Document.audio` を直接使う検査は実物のままである。
- **テスト名**: 代役は文書の作業スレッドで呼ばれ、どのテストの要求かを知らない。Swift Testing の issue にも「unknown」と出る。
  `make check-real-hal` は全テストを `--no-parallel` で実行し、イベント列からその時に実行中のテストを出す。
- **束縛の確認**: 検査は最初に `nm -u` で、バンドルに実物の参照がないことと、製品に実物が残っていることを確かめる。
  新しい HAL の関数を Rust で使うと、ここで止まる。
- **正の対照**: `RealHALGuardTests` は環境変数 `ECHODE_REAL_HAL_CONTROL` がある時だけ動き、既定の `ProjectController()` で再生を要求する。
  検査はこれが名指しで止まることを毎回要求し、代役が効かなくなったのに合格することを防ぐ。
  対照が再生を要求するので、束縛の確認を必ず先に済ませる。

## 測定

MacBook Air M4、2026-11-01。

| 確認 | 結果 |
| --- | --- |
| 現在の全テスト（266 件）を代役つきで実行 | 実 HAL に届いたテストは 0 件 |
| 全体実行の時間 | 並列 27 秒、直列 33 秒 |
| `FolderDisclosureTests` の注入だけを外す | `make check-real-hal` がそのテストの名前と `AudioObjectGetPropertyData on echode-worker` を示して失敗 |
| 注入と窓要求のアサーションを外し、並列の全体実行を 5 回 | 5 回とも代役の報告で失敗。ほかのアサーションの失敗は 0 件 |
| 一時的に `controller.audition` を対照から呼ぶ | `AudioObjectGetPropertyData on echode-audition` で失敗 |
| `nm` の結果 | バンドルは 9 関数を定義し、未定義は時計の 2 関数だけ。`EchodeAppProbe` は 9 関数とも未定義（実物へ束縛） |

## 限界

- システムのフレームワークが内部で HAL を呼ぶ経路は置き換えられない。今は Swift から使っておらず、使うなら別の対策が要る。
- テスト名は「その時に実行中のテスト」である。
  前のテストが待たずに残した要求が次のテストの実行中に届くと名前がずれるので、検査は直前のテストも併記する。
- 並列の実行では失敗はするが、テスト名は出ない。`make check-real-hal` で特定する。
- `write_planes` の TSan 警告の原因は未確定のままである。この決定は UI テストからその経路をなくすもので、原因の解決ではない。
