# ADR 0334: 出力を閉じる・機器を替える時は、起動中のヘルパーを待たずに取り消す

日付: 2026-11-13
状態: 採用。実機の操作感はユーザー確認待ち（UC-70）
関連: [ADR 0136](0136-output-opening-off-worker.md)、[ADR 0142](0142-background-plugin-graph-preparation.md)、[ADR 0199](0199-owned-helper-control-requests.md)、[ADR 0276](0276-priority-order-and-delegated-decisions.md)

## 問題と再現

開始時の HEAD は `6f02c1f`。P2-02 の応答性の残り（[進め方](../phase2.md)）のうち「終了時の初期化待ちの中断」を扱った。

出力の最初の準備（`echode-output-open`、ADR 0136）と置換の準備（`echode-graph-prepare`、ADR 0142）は、ヘルパーを 1 つずつ起動する。
1 つの起動は、ポートの報告と準備完了をそれぞれ 5 秒まで待つ（`PluginHost::spawn`）。
文書を閉じる時・機器を替える時・出力を閉じる時は、`Opening` と `GraphPreparation` の `Drop` がこのスレッドの終わりを待っていた。
初期化の遅いプラグインを多く含む曲を開いた直後に閉じると、全部の起動が終わるまで閉じ終わらない。
機器の変更で置換準備中の出力を閉じる時は、その待ちが文書ワーカーの手番で起きる。

実ヘルパーの初期化を `prepare-wait` で保留して再現した。取り消しの確認をなくすと、置換と最初の準備の新しい回帰はどちらも、
起動自身の 5 秒の期限で `Spawn { error: Timeout }` になり、`expected a cancelled start` で失敗した（終了コード 101、`target/focused-checks/helper-cancel/red.log`）。
`Opening` の `Drop` で取り消さないと、保留したまま閉じる Session の回帰が `closing waited for the held preparation` で失敗した（`target/focused-checks/helper-cancel/session-red.log`）。

## 比べた案

1. **待ち続ける（従来）**: 単純だが、閉じる・機器交換の待ちがプラグインの数と初期化時間に比例する。
2. **スレッドを切り離して待たない**: 閉じるのはすぐ終わる。代わりに不要なヘルパーが初期化を続けて資源を使い、終わりを誰も確かめない。
   アプリの終了時に残る子プロセスの扱いも、親の終了の検知に頼る。
3. **別スレッドから PID でヘルパーを止める**: 起動を待つスレッドが子を回収した後に PID が再利用されると、無関係なプロセスを止め得る。
4. **取消の印を起動の待ちが自分で見て、自分の子を止める（採用）**: 待つスレッドだけが子を止めて回収するので PID の競合がない。
   待ちは 50 ms（ポートの報告）・20 ms（準備完了）ごとに印を見る。印を見たら子を止め、`SessionError::Cancelled` を返す。

## 決定

- `SpawnOptions::cancel`（`Option<&AtomicBool>`）を足した。起動の待ちは印を見たら子を止め、`SessionError::Cancelled` で失敗する。
  クラッシュ後の再起動は持ち主がいないので、印を渡さない。
- `ReconcileRoutes::cancel` と `PlaybackOptions::cancel` で印を `HostGraph` へ渡す。次のヘルパーの起動前にも印を見て、
  始めた分を従来の取消（`cancel_preparation` のチェックポイント）で止め、`PlaybackError::Cancelled` を返す。
  `SessionError::is_cancelled` が真の時だけ `Cancelled` とし、起動自身の期限切れは従来どおり `Spawn` のまま区別する。
- 置換の `GraphPreparation` は自分の印を持ち、`Drop` で印を立ててから終わりを待つ。`PlaybackControl::cancel_preparation` も同じ印を立てる。
  `OutputStream::close` はこれを最初に呼ぶ。通常の `service` は終わったスレッドしか落とさないので、印は効かない。
- 最初の準備は `TransportSetup::cancel` を `Opening` と準備スレッドで共有する。`Opening` の `Drop` と、機器の変更で退ける時（`retiring`）に印を立てる。
- 取り消した準備の結果（`Cancelled`）は捨てる。置換を取り消しても、公開済みのグラフと既存のヘルパーはそのまま鳴る。
- 保存済み状態の読込み（2 秒）と状態の復元は取り消さない。どちらも期限を持つ。

## 検査

- 必須契約 `helper-start-cancel`: 置換で初期化を保留した実ヘルパーを取り消すと、期限を待たずに `Cancelled` で終わり、
  保留中のヘルパーが止まり（初期化の相手の接続が閉じる）、既存のヘルパーが残る。負の対照は起動の待ちの印の確認をなくす。
- 必須契約 `helper-open-cancel`: 最初の準備（`playback` と `PlaybackOptions::cancel`）で同じことを確かめる。負の対照は印を `HostGraph::open` へ渡さない。
- 必須契約 `output-close-cancel`: 機器なしの出力の準備を保留したまま Session を落とすと、保留を解かずに閉じ終わる。
  `HeadlessOutput` の保留は、ヘルパーの起動の代役として同じ印で終わる。負の対照は `Opening` の `Drop` で印を立てない。
- 判定は結果の種類と順序で行い、時間の閾値は使わない。10 秒の待ちは止まった時の保険である。
- RT 区間と DSP は変えない。

## 影響と残り

- 閉じる・機器交換・出力の終了は、起動中のヘルパーを最大でも約 50 ms で止めて先へ進む。
- 置換準備専用のデバイス列の表示は未接続のまま（ADR 0142）。
- 計画生成は文書ワーカーに残る（[ADR 0333](0333-install-diff-graph-off-worker.md)）。
