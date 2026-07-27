# Knot UI 完全再現 — gap ログ

**目的**: React/Tailwind 版 Knot (`knot-notes-app-v0.7.0-2026-04-27`) を「正解画面」とし、
sindon で見た目を写経する。写し取れない / 不格好になる箇所を gap として収集する。
機能 dogfood (FW-1〜14) が「動くか」を見てきたのに対し、これは **表現力 (見た目) の天井** を炙る。

- 再現先: `examples/knot_clone`（UI-only。crypto/db なし、ダミーデータ）
- 正解参照: React ソース + Tailwind パレット（ビルドせず静的に spec 化）
- 確定した gap は後で FW-15+ として `docs/dogfood-log.md` に graduate する

凡例: **重大** = 忠実再現を妨げる / **中** = 回避策はあるが歪む / **小** = 些細

---

## 画面別 進捗

| 画面 | 状態 |
|------|------|
| Unlock | 完了（gap 由来の差を除き React 版と一致） |
| Setup | 完了（slice 5: ヘッダ/見出し/マスターPW欄+強度メーター/確認欄/リカバリーKey チェック/送信。**新規 gap なし** — 既存 G5/G3/G6/G12 のみ） |
| Recovery | 完了（slice 6: ヘッダ/見出し/リカバリーキー textarea/新PW/確認PW/送信/Back リンク。**新規 gap なし** — 既存 G3/G6/G12 のみ、textarea の固定高だけ G3 系に軽く追記） |
| Main — Sidebar shell | 完了（slice 1: パネル/ヘッダ/New Note/検索/ノート行） |
| Main — Editor pane | 完了（slice 2: タイトル欄/操作ボタン/タグ/ツールバー/本文/ステータスバー） |
| Main — Overlays (設定 dropdown / エラーバナー / context menu) | 完了（slice 3: 設定/⋮ dropdown・右クリック context menu・エラーバナー。**G5 検証台** — 下記） |
| 各種 Modal (Backup/ChangePw/Restore + 共通 ConfirmDialog) | 完了（slice 4: scrim+centered card を `LayerOptions::modal` で。3段スタック(Backup→Restore→Confirm)・sticky header/scroll/footer・checkbox・strength meter・grid 近似。**G18(shadow 無) 発見 + G19(layer clip オフセット未適用) 実バグ発見・修正** — 下記） |
| Loading | 完了（slice 7: 中央 "Knot" + "Loading..." のみ。スピナー無し・新規 gap なし） |

---

## Gap 一覧

### G1. Container に border がない → **解消（FW-15、2026-06-29 landed）**
- 正解では border が遍在する: input (`border border-gray-300`)、エラー枠 (`border-red-300`)、
  カード、テーブルセル、サイドバー区切り、select。
- ~~sindon の `Container` は `background` + `radius` のみで枠線を描けない。~~
- 対応: `Container::border(width, color)` を追加。FW-14 の Input border と同じ
  `stroke_rect_rounded` SDF 基盤を流用。`radius` で角丸し、塗りの上に重ねて描く
  ので透明ボックスも outline だけ持てる。color は `Reactive<Color>` なので live theme
  追従。`width <= 0.0` は no-border（既定）。テスト6本 (`container_border_*`)。

### G2. `SecureInput` に chrome カスタマイズがない（radius/border/borderless）→ **解消（FW-15、2026-06-29 landed）**
- ~~FW-14 で `Input` には radius/border_color/borderless が載ったが SecureInput は未反映。~~
- 対応: FW-14 の chrome を `SecureInput` に対称展開 — `radius` / `border_color` /
  `borderless` を追加。旧式の4矩形 inset border を Input と同じ
  `fill_rect_rounded` + `stroke_rect_rounded` 1ストロークに置換。focus ring も
  `radius` 追従にした（角丸欄に角丸リング）。**おまけ**: 同じ非対称を抱えていた
  `Input` の focus ring も `0.0` 固定 → `self.radius` に揃えた。テスト5本
  (`secure_input_*_chrome` 系)。Unlock のパスワード欄は `.radius(8.0)` だけで
  `rounded-lg border-gray-300` に（border は既定で `input_border` 追従）。

### G3. Input / SecureInput の padding・高さが非公開 → **解消（FW-17、2026-07-02 landed）**
- 正解の入力欄は `px-4 py-3`（≒ 高さ 48px）。~~sindon の Input/SecureInput は内部 padding 固定で
  寸法を合わせられない。~~ボタンも同様（`py-3` → G6 で別途）。
- ~~実装裏付け（`input.rs:1469`）: 単一行 Input は `FlexStyle::new().padding(8.0)
  .min_height(font_size + 20.0)` を**ハードコード**。~~
- 対応: Input / SecureInput に `padding_x(px)`（左右インセット、Tailwind `px-*`）/ `padding_y(px)`
  （上下インセット、`py-*`）/ `min_height(px)`（font・行数由来 floor の明示上書き）を対称追加。
  paint 側のハードコード（`text_x` / `max_width` / multiline の `text_y` / `viewport_h` /
  scrollbar / wrap 幅）を全部この値に置換。**デフォルト（pad 8）はビット等価**なので既存レイアウトは
  不変。widgets/secure 各 +4 テスト。↓の実害はすべて実 API で解消（下記「clone 配線」）。
- **実害例（Main slice 1 — 検索バーが「妙に縦長」）**: React は「枠付き div（`px-3 py-2`）+
  bare な borderless input」で ≈36px。sindon で同じ構成にすると「枠付きコンテナ padding 8 +
  borderless Input（自前 padding 8 + min_height 34）」= **≈50px** に膨らむ。`borderless()` は
  *枠*を消すだけで*内部 padding と min_height は残る*ため、カスタム chrome 行に Input を
  畳み込むと縦 padding が二重になる。
  - しかもこの1要素が G3 と **G4 を同時に踏む**: uniform padding なので「縦を詰める（コンテナ
    padding を下げる）」と「🔍 アイコンの左インセット」がトレードオフになり両立しない。
  - ~~clone の暫定: コンテナ padding を 8→4 に下げて高さを ≈42px に寄せた。~~
    **解消**: input を `padding_x(0).padding_y(0).min_height(20)` にゼロ化し、行の `py-2`
    (`padding(8)`) に高さを委譲 → ≈36px。アイコン↔テキストの間隔は行の `gap(8)` が持つので
    「縦を詰めると左インセットも縮む」トレードオフ自体が消えた。
- **実害例 2（Main slice 2 — Editor 本文/タイトル）→ 解消**: 本文は `padding_x(24).padding_y(16)`
  で `padding:16px 24px` を直接表現（旧・左 inset スペーサ削除）。タイトルは `min_height(32)` で
  `text-2xl`（2rem）の行箱に（font-derived 44px → 32px）。※太字だけは G12（Input weight）待ちで未。
- **clone 配線（FW-17 実使用）**: Unlock パスワード `padding_x(16).min_height(48)`（`px-4 py-3`）/
  検索バー（上記）/ 本文・タイトル（上記）。**実機 OK**（2026-07-02 ユーザ確認、light/dark とも自然）。

### G4. `padding` が上下左右一律のみ → **解消（Input/SecureInput = FW-17、Container = FW-19、2026-07-03 landed）**
- Tailwind は `px-4 py-3` / `px-2 py-1` のような非対称 padding が常用。
- **Input/SecureInput**: `padding_x`/`padding_y`（FW-17）で軸別に指定可（`px-4 py-3` 等）。
- **Container**: `Container::padding_xy(x, y)`（`px-* py-*`）+ `Container::padding_trbl(top, right,
  bottom, left)`（全辺独立）を公開（FW-19）。内部 `FlexStyle::padding_trbl` の薄いラッパで負値クランプ。
  clone のエディタタイトル節は uniform `padding(16)`＋子の 8px inset の近似 →
  真の `padding_xy(24, 16)`（`px-6 py-4`）へ置換し、補正スペーサを撤去（右端 8px ズレも解消）。

### G5. absolute / コーナー配置のプリミティブがない → **解消（FW-21、2026-07-04 landed）**
- **3つの open 点をまとめて graduate**（下の検証で炙った3点すべて）:
  - **① click 時に trigger の rect が取れない → 解消**: `Container::on_press_rect(FnMut(Rect, ctx))`
    を追加（`on_hover_enter` の press 版）。`MouseDown` で widget 自身の `layout` rect を渡し consume。
    `on_press`（点）と併用可・両方 fire。layer 内では rect は layer-local で、`push_layer` の既存
    offset 変換（G14）でそのまま viewport 化される。
  - **② 右寄せ placement が無い → 解消**: `LayerAnchor::AnchorRect` に `align: HAlign` を追加。
    `HAlign::{Start,Center,End}` = popover 左辺を trigger 左辺 / 中央 / 右辺（CSS `right-0`、左に開く）に
    合わせる。`place_layer` の x 計算を align 分岐に。
  - **③ 固定エッジ / オフセット anchor が無い → 解消**: 新変種 `LayerAnchor::Viewport { h: HAlign,
    v: VAlign, offset: (f32,f32) }`。viewport の角/辺 + ピクセル nudge で CSS `absolute` 相当。
    バナー `top-2 left-1/2 -translate-x-1/2` = `Viewport { Center, Start, offset: (0,8) }`。viewport
    絶対ゆえ `push_layer` は変換しない（`ViewportCenter` と同様）。`ViewportCenter` は
    `Viewport{Center,Center,(0,0)}` の名前付きショートハンドとして温存。
- **clone 配線**: gear/⋮ ヘッダメニューを `on_press_rect` + `AnchorRect{align: End}` にして React の
  `right-0 top-full`（ボタンに anchor・右寄せ・下開き）を 1:1 に。context menu は cursor anchor のまま
  （正しい）。エラーバナーを `Viewport{Center, Start, offset:(128,8)}` に（x=128 = サイドバー 256px の
  半分 → エディタペイン中央、React の pane-relative `left-1/2` と一致。y=8 = `top-2`）。
- **破壊的変更**: `AnchorRect` にフィールド追加のため全構築サイト（widgets/dropdown・tests・
  knot sidebar/tooltip・knot_spike）に `align: HAlign::Start` を機械的追記（挙動不変）。knot は
  この1点のみ変更（enum が育った正当な source break）。
- テスト: layer +4（right/center/viewport top-center/viewport bottom-right）・event +2
  （align が offset 変換を跨いで保持 / Viewport 非変換）・press_blur +1（on_press_rect が rect を返す）。
  全 gate 緑（fmt/clippy 含 knot/rustdoc -D warnings/全 widget test）。
- **実機 OK**（light、2026-07-04 ユーザ確認 — 設定メニューが gear に anchor・エラーバナーがエディタペイン上端中央。
  HDR ∴ 色はユーザの目が正、[[feedback-screenshot-color-hdr]]）。

<details><summary>（旧記載：slice 3 での検証内容）</summary>

**中（slice 3 で検証完了 = 部分的に Layer で代替可）**
- Unlock 右上の言語 select は `absolute top-4 right-4`。通常フローの外に置く手段が
  Container 系にない（`Layer` + anchor で代替可能か要検証）。第一稿では言語 select を省略。
- **slice 3 検証（2026-06-29）**: Main の overlay 4種（設定 dropdown / ⋮ actions / 右クリック
  context menu / エラーバナー）を `Layer` で写経し、`absolute`/`fixed` をどこまで代替できるか確定した。
  - **✓ カーソル位置 anchor の popover は綺麗に再現可**。context menu は `Container::on_context_menu`
    が渡すクリック点を `LayerAnchor::AnchorRect { rect: Rect::new(pos.x, pos.y, 0,0), Below }` に
    流すだけで React と 1:1。これが本命の確認 (= overlay は概ね Layer で賄える)。
  - **✗ click 時に trigger 自身の rect を取る手段が無い**。dropdown を自分のボタンに anchor する
    (`right-0 top-full`) のは普遍的 UX だが、trigger の rect を返すのは `Container::on_hover_enter`
    (tooltip 経路) **だけ**。click/press ハンドラは*カーソル点*しか渡さない (`on_press`/
    `on_context_menu` の `Point`)。∴ ボタン anchor の dropdown は (a) カーソル anchor で近似するか
    (b) `Dropdown` のように内部で `event(layout)` を使う custom Widget を書くしかない。clone は (a)。
    → 候補: `Container::on_click`(rect 付き) ないし `on_press` に rect も渡す変種。
  - **✗ 右寄せ placement が無い**。`AnchorRect` は popover の*左*辺を `rect.x` に合わせる
    (+viewport クランプ)。React の `right-0`(右辺を trigger 右辺に) は placement に無いので、
    gear/⋮ メニューは gear の**右**に開く (React は左に開く)。→ 候補: `Placement` に水平方向
    (`AlignLeft`/`AlignRight`) を追加、または `AnchorRect` に align フィールド。
  - **✗✗ 固定エッジ / オフセット anchor が無い（最重要）**。エラーバナーは
    `absolute top-2 left-1/2 -translate-x-1/2`(エディタペイン上端中央)。`LayerAnchor` は
    `ViewportCenter`(両軸中央) と `AnchorRect`(rect 相対) のみで、**top-center も任意の viewport
    オフセットも表現不可**。clone は styling 確認のため `ViewportCenter` で出した（位置は画面中央
    で誤り）。→ `LayerAnchor` は `#[non_exhaustive]` で「absolute anchor は後で追加可」と既に明記
    しており、これがその変種 (`Viewport { align, offset }` 等) を入れる最有力の動機。
  - **付随する小 gap（slice 3 で同時発見）**:
    - `MenuItem` に disabled 状態が無い（actions の "Export All" は `disabled:opacity-40`)。
    - `Dropdown` に `grow` が無い（sort 行の select は `flex-1`）。
    - `Container` に `min_width` builder が無い（context menu `min-w-[140px]` は固定 `width` で代用)。

</details>

### G6. Button の padding / 高さ / 固定幅 / disabled スタイルが非公開 → **解消（FW-19、2026-07-03 landed）**
- 正解の submit は `w-full py-3` かつ `disabled:bg-blue-800 disabled:cursor-not-allowed`。
- ~~Button は `radius`/`background`/`text_color`/…/`grow` はあるが padding・高さ・固定幅・disabled が無い。~~
- 対応: `Button::padding_x/y`（既定 8 でビット等価、ハードコード `padding(8)` を置換。`py-3` は
  `padding_y(12)`）/ `min_width`（**measured-leaf 不変条件**を守り style `min_size` でなく `measure`
  側で content を `min_width - 2*pad_x` に floor → 単一グリフ icon ボタンを均一化）/ `disabled(Reactive<bool>)`
  （hover/press フェード停止・`focusable()=false` で Tab 除外・`on_click` 不発）/ `disabled_background`
  （既定は通常背景+ラベルをアルファ半減、テーマ非依存の greyed-out）。`w-full` は従来どおり flex stretch。
- 残: `cursor: not-allowed`（カーソル形状 API 自体が無い）は非対象。アイコン均一化の根因の半分は
  アイコンフォント未同梱（FW-12、本 clone の対象外）だが、`min_width` で幅は揃えられるようになった。

### G7. focus モデルの差（外側リング vs border 色変化）→ **解消（FW-26、2026-07-08 landed・実機 OK）**
- 正解は `focus:outline-none focus:border-blue-500`＝**枠線の色が変わるだけ**。
- ~~sindon は外側に focus ring を描く（offset 付き）ので見た目の質感が異なる。~~
- 「どちらを既定にするか」は設計判断だったので、**テーマレベルで選べる**ようにして graduate（ユーザ選択）。
- 対応: `FocusStyle` に `indicator: FocusIndicator { Ring（既定）/ Border }` を追加（dark/light とも
  `Ring` 既定 ＝ 既存挙動ビット等価・非破壊。lerp は指標 snap）。`Border` モードでは **border を持つ
  widget（`Input` / `SecureInput` / `Dropdown`）** が focus-visible 時に自分の 1px border を focus 色
  （per-widget `focus_ring_color` override があればそれ、無ければ `focus.ring_color`）に recolor し、
  リングは描かない。色ソースを `focus.ring_color` に一本化したので、既存の `focus_ring_color` override が
  両モードで効く。
- ~~**リングの `:focus-visible` ゲート（キーボード／プログラム由来のフォーカスだけ表示）は従来どおり効く**
  ので、`Border` モードでも「クリック focus では枠が変わらない」挙動は保たれる。~~
  → **FW-27（2026-07-08）で訂正**。web の `:focus-visible` は一律ではなく、**テキスト入力欄（`<input>`/
  `<textarea>`）はマウスクリックでも常に focus-visible**（キャレット位置を見せる必要がある）。コマンド系
  （Button/link）だけ keyboard 由来に限定される。∴ `Input`/`SecureInput` は **focused なら常に指標を出す**
  （`focus_active = self.focused`、Border も Ring もクリックで点灯）ように直し、Web と 1:1 に。
  **Button/Checkbox/Dropdown は `:focus-visible` ゲート据え置き**（コマンド系=毎クリックの表示はノイズ）。
- **非対象の fallback**: recolor する border を持たない `Button` / `Checkbox`、および `borderless()` な
  入力は、`Border` モードでも**リングに fallback**（focus 表示を失わない。Web も入力=border/ボタン=ring の
  mixed model なので Knot 再現としても正しい）。
- テスト: theme +2 / widgets +6 / secure +2 / dropdown +1（= +11）。全 gate 緑。
- clone 配線: [tokens.rs](../examples/knot_clone/src/tokens.rs) の light/dark を `focus.indicator = Border`
  ＋ `focus.ring_color = blue_500()` に。入力欄のリングが実 border 変化になり `focus:border-blue-500` に 1:1。
- **実機 OK（2026-07-08 ユーザ確認）**: master password 欄を Tab フォーカスすると枠が blue-500 に変わる
  （リングは出ない）のを確認＝ SecureInput 経路の実機確認（HDR ∴ 色はユーザの目が正、[[feedback-screenshot-color-hdr]]）。

### G8. ~~sRGB hex で色が洗い出される（ガンマ非対応）~~ → **誤検知（取り下げ）**
- 第一稿のスクショで全色が洗い出されて見えたが、**原因は描画ではなく HDR→SDR スクショ側の
  アーティファクト**。ユーザの実機（HDR 環境）では `from_rgba8` がそのまま正しく描画されており、
  HDR を切ったら素の `from_rgba8` で自然な色になった。
- 一度入れた `tokens::s2l()`（sRGB→linear デコード）は**過補正**なので撤去済み。
- 結論: **この「真っ白スクショ」自体は HDR キャプチャ artifact**。原因は Claude Desktop の
  PC 操作キャプチャが HDR 非対応で白飛びするだけで、描画とは無関係。
- ⚠ **方法論メモ**: この PC では**スクショの色は当てにならない**（HDR 起因）。以後スクショは
  **レイアウト/構造判定専用**にし、**色の忠実度はユーザの目を正**とする。framework の色バグを
  スクショだけで断定しない。
- ⚠ **訂正（2026-06-29）**: 当初ここで「sindon に色空間バグは無い」と結論したのは**勇み足**だった。
  HDR スクショが不可信で色判定を保留した隙に、**実在の二重ガンマ描画バグ（G13）を見逃していた**。
  G8（HDR キャプチャ）と G13（描画の二重ガンマ）は**別問題**。皮肉にも「色はユーザの目を正」と
  いう G8 の方法論があったから、HDR を切った実画面で G13 を炙り出せた。

### G9. 入力欄の枠が弱い（未フォーカスでほぼ枠なし）→ **解消（G1/G2 の派生）**
- ~~未フォーカスで枠がごく薄く、border/radius ビルダーが無いので常時枠を意図的に付けられない。~~
- G2 で `SecureInput` / `Input` とも border が既定 ON（`input_border` 追従）+ `radius`
  対応になったので、未フォーカスでも常時 `border border-gray-300 rounded-lg` 相当が出る。
- 残っていた差は G7（フォーカス時に「外側リング」が出る vs 正解は「枠線の色が変わるだけ」）→
  **FW-26（2026-07-08）でテーマ `FocusIndicator::Border` として解消**。`Border` モードで未フォーカス〜
  フォーカスとも `border border-gray-300 rounded-lg` → `focus:border-blue-500` の遷移が 1:1 になった。

### G10. 片側 border (`border-r` / `border-b`) が引けない → **解消（FW-16、2026-07-02 landed）**
- 正解は仕切り線を片側 border で多用する: サイドバー右端 `border-r`、ヘッダ／検索／タグ各
  セクション下端 `border-b`、dropdown 内の区切り。
- ~~sindon の `Container::border(width, color)`（FW-15）は**4辺一括のみ**。1辺だけの線が引けない。~~
- 対応: `Container::border_top/right/bottom/left(width, color)` を追加。各辺を sharp な
  `fill_rect` で描画（Tailwind `border-r`/`border-b` 直対応）、color は `Reactive<Color>` で
  live theme 追従、`width<=0` は無描画、4辺 `border()` とは独立。clone の 1px divider 兄弟は
  実 `border_*` に置換（`divider` fn 削除）。popover 行間の区切りだけは兄弟 divider を温存
  （別ヘルパで作る行の**間**に入るので box の辺にならない）。
- **★ この実装中に framework 実バグ発見・同時修正（G17）**: 片側 border（や4辺 `border()`）が
  full-bleed な子背景に上書きされて消える問題。詳細は G17。

### G11. flex 整列が `center` 系しかない（`justify-between` / `*-end` / `*-start` 不可）→ **解消（FW-16、2026-07-02 landed）**
- 正解のヘッダ行は `flex items-center justify-between`（タイトル左・操作ボタン群右）。行・
  列の両端寄せ・端寄せが頻出（モーダルのフッタボタン、リスト行の右端メタ等）。
- ~~sindon は `center` / `justify_center`（主軸中央）/ `align_center`（交差軸中央）のみ。~~
- 対応: sindon ネイティブの `Justify`（Start/Center/End/SpaceBetween/SpaceAround/SpaceEvenly）
  / `Align`（Start/Center/End/Stretch）enum を `sindon_layout` に追加 →
  `FlexStyle::justify/align` + `Container::justify/align`。taffy を widget API に漏らさず
  `From` で内部マッピング（既存 center 系は温存）。clone のヘッダ `justify-between`・ステータス
  `text-right` は `grow(1.0)` スペーサ → `justify(SpaceBetween)`/`justify(End)` に置換。
  （ツールバーの `flex-1` スペーサは React も本物の spacer なので `grow` のまま。）

### G12. `Input` に font-weight（太字）が無い + chrome setter が非 Reactive → **解消（FW-19、2026-07-03 landed）**
- 正解の Editor タイトル欄は `text-2xl font-bold`。~~`Input` には `font_size` しか無く太字にできない。~~
- 対応: `Input::weight(FontWeight)`。**要点は「配線のみ」ではなく caret/hit-test/選択の幾何も同じ
  weight で整形する**こと（色だけの highlight と違い bold は advance が変わるので、plain 整形の幾何を
  流用すると caret がズレる）。text engine に attrs 版を追加（`offset_at_point_attrs` /
  `selection_rects_attrs`(+`_with_trailing_attrs`) / `caret_at_offset_attrs`。既存メソッドは
  `TextAttrs::default()` へ委譲＝挙動不変）。clone のタイトルは `font_size(24).weight(BOLD)` に。
- 併せて **chrome setter を Reactive 化**: `Input::background` / `border_color` / `text_color` /
  `focus_ring_color` / `selection_color` を `Color` → `impl Into<Reactive<Color>>`（Container/Button と
  対称、live テーマ追従）。`Color` は `Into` で従来呼び出しと source 互換 ∴ knot 無改変。
- 非対象: `SecureInput` の weight は mask ゆえ無意味、chrome-Reactive も Input のみ先行。

### G13. 色が全体的に淡い → **二重ガンマ（framework 描画バグ）→ 解消（2026-06-29 landed）**
- 症状: ライト/ダーク両方で色が washed・低コントラスト（特にダークで `gray-900` 背景が灰色に浮く）。
  **HDR を切っても残る**ので G8（HDR スクショ artifact）とは**別問題**＝実画面の実バグ。
- 真因（端から端まで追跡）: サーフェスは sRGB 形式（`renderer.rs` の `.find(|f| f.is_srgb())`）。
  GPU は fragment 出力を **linear** とみなし linear→sRGB で書き込む前提。だが `Color::from_rgba8`
  は sRGB 値（hex/255）を**そのまま**保持（`geometry.rs:107`）、rect/text/image シェーダは
  `in.color.rgb` を**無変換出力** → **sRGB を二重エンコード**して持ち上がる（`#11`=0.067 が ≈0.27）。
  暗色ほど顕著。
- 修正: 3 シェーダ（`RECT_/TEXT_/IMAGE_SHADER`）の fragment 出力直前に WGSL `srgb_to_linear()` を
  噛ませて linear 化（CPU の image mip 用 `srgb_to_linear` と同式: `0.04045` 境界・`2.4` 乗）。sRGB
  再エンコードが恒等になり解消。**ブレンドも linear 空間になりテキスト AA も締まる**。image は texel が
  既に linear（sRGB テクスチャ sampler）なので tint のみ変換。`Color`/`to_array` の CPU 側
  （lerp/テーマ/アニメ）は無改変なので安全。
- 検証: clippy/fmt/全テスト緑、WGSL はランタイム検証 → 起動でパニックなし。clone + **knot 本体**とも
  実機 light/dark 正常（ユーザ確認、knot も「予想よりずっと暗い」＝正しく濃くなった）。
- 教訓: この repro 演習（密な実画面 + ユーザの目）でないと炙れなかった種類のバグ。framework 全体に
  効く最大級の収穫。

### G14. layer 内の widget から開く `AnchorRect` popover が offset 分ズレる → **framework 実バグ → 解消（2026-06-29 landed）**
- 症状（slice 3、実機）: 設定 dropdown（layer）の中の Theme `<select>`（`Dropdown`）を開くと、
  選択肢リストが **trigger の下ではなく画面左上**（≈ layer-local 原点）に飛ぶ。`Dropdown` を modal/
  popover の**中**に置くと必ず再現。main tree 直下の `Dropdown` は正常。
- 真因（端から端まで追跡）: 各 layer は `WidgetTree::compute_layout*` で
  `self.layout.compute(layer_root, w, h)`（`tree.rs:799`）と **layer 自身の原点基準**に独立レイアウト
  され、viewport への `offset` は `place_layer` で別途算出して `layer.offset` に保持、paint/dispatch
  時に加算する設計。ところが `dispatch_to_node`（`tree.rs:1811/1825`）は `widget.event()` に
  `self.layout.absolute_rect(node.layout_node)` = **layer-local の rect** をそのまま渡す。`Dropdown`
  はその `event` の `layout` を `LayerAnchor::AnchorRect{ rect }`（**viewport 座標期待**）へ素通しする
  （`dropdown.rs:160-184`）ので、子 popover が layer offset 分だけ左上にズレる。`top_layer_offset`
  の doc 自身が「layer の子 rect は local。viewport にするには offset を足せ」と認めている
  （`tree.rs:367-374`）が、widget 側にその offset を知る手段が無い。
- 派生症状: 「設定を閉じるのに外側を2回クリック要る」= ズレた選択肢リストも layer なので、stack に
  2枚（設定 + 選択肢）積まれ、1クリック=選択肢を閉じ・2クリック目=設定を閉じる、という当然挙動が
  「謎の2回」に見えていただけ（G14 が主因）。
- 修正案: (a) layer subtree の dispatch で `event` に渡す `layout` に layer offset を足して
  viewport 化（cursor 側は既に offset 減算済みなので hit-test との座標系整合に注意）、または
  (b) `EventContext::layer_offset()` を公開し widget 側で `layout.translate(offset)` してから anchor。
  どちらも座標系の機微があり、[[feedback-test-translation-layer]] 通り platform 変換層も独立 test 要。
- 影響範囲: 「modal/popover の中の Dropdown」「サブメニュー」全般。app 作者が自然に踏む。**G13 級の
  framework 全体バグ**。
- **修正（landed）**: 案 (b) を採用。`EventContext` に dispatch 中の `current_layer_offset` を持たせ
  （`dispatch_event` が active layer の offset から設定・dispatch 末尾で `(0,0)` にリセット）、
  `EventContext::push_layer` が `AnchorRect` の rect をその offset で viewport 化する。widget 側は
  無改変（`Dropdown`/`on_context_menu` とも「自分が受け取った local rect をそのまま渡す」ままで正しく
  なる）。main tree は offset 0 ＝ no-op で回帰なし。`event.rs` にユニットテスト3本（layer 内で平行移動 /
  main tree で不変 / `ViewportCenter` は不変）。`layer.rs` の `AnchorRect` doc も「rect は push する
  ハンドラの座標系。layer 内なら自動で viewport 化」と更新。clone の設定 select は実 `Dropdown` に復帰。

### G15. interactive layer push が trigger の `MouseLeave` を握り潰し、ホバーが固定される → **framework 実バグ → 解消（FW-23、2026-07-06 landed）**
- 症状（slice 3、実機 + ユーザ指摘）: クリックで popover を開く trigger（`hoverable` な
  `Container`、gear/⋮ 等）が、popover を閉じた後も**ホバー色（灰）のまま固定**。もう一度ホバーし
  直すと直る（実害は軽微だが「全ボタンをホバー状態で固定できてしまう」違和感）。
- **真因（診断で判明）**: hover の「見た目（`Container` の `hover_anim`）」は **`MouseLeave` を受けた時だけ**
  0.0 に戻る。ところが interactive layer を push する箇所（`tree.rs` `push_layer_boxed`）が
  `self.hovered` を **`clear_hover` でなく直接 `= None` 代入**していて、chain に `MouseLeave` を一切撒かない。
  → trigger の `hover_anim` が 1.0 に孤児化して固定。**hover の「見た目」と「状態（`self.hovered`）」が
  layer 境界で desync** する severance が核。
- **旧「棚上げ」診断の誤り**: かつて「push 時点で `self.hovered` は毎回 `None`」と観測して naive clear を
  no-op と結論し棚上げしていた。今回 probe テスト（`g15_..._clears_trigger_hover`）で確定 →
  `entered=1, exited=0, hovered=None`（push 後）＝ **push 直前まで `hovered=Some(trigger)` は live**、
  silent null が leave を握り潰していただけ。旧診断は計測位置ミスだった。
- **修正（Option A・最小）**: `apply_commands` の `PushLayer` arm で、interactive なら
  push（＝pointer null）の**直前**に `self.clear_hover(event_ctx)` を呼び、live な hover chain へ
  `MouseLeave` を leaf-first に撒く。`event_ctx` を持つのは drain/event 経路だけなので `push_layer_boxed`
  内でなくここで行う（boot/test 経路は hover 自体が None ∴ 無関係）。`push_layer_boxed` の silent null は
  boot 用 baseline としてそのまま残置（event 経路では既に None ＝ no-op）。
- **非 interactive は据え置き**: `if options.interactive` gate により tooltip（click-through）layer は
  hover を消さない。FW-13 の「非 interactive は入力を main tree に残す ∴ hover 継続」契約を保持
  （消すと次 move で spurious re-enter → tip 再オープン）。
- **テスト**: `layer_tests.rs` に2本 — interactive push で trigger が leave を受け `hover_anim` reset
  （`exited==1`/`hovered==None`）/ 非 interactive（tooltip）push は hover 維持（`hovered==Some(trigger)`/
  `exited==0`）。sindon_widgets 全 test 緑・fmt/clippy クリーン。**実機 OK（2026-07-06 ユーザ確認 —
  メニュー開閉で trigger のホバー灰が固定しないこと確認）。**
- **副次の非対称（かつて acceptable と記録 → ✅ FW-31、2026-07-10 で解消）**: 「pop 側の非対称」。
  ①メニューを開いている間は layer が入力を独占するので**メニュー外の hover は出ない**（＝正・input
  priority、今も不変）。②メニューを**別ボタンのクリックで閉じた**とき、カーソル下の widget は**次に
  マウスを動かすまで hover 表示されない**。次 move で自己修復するため当初は acceptable と記録したが、
  実使用で繰り返し気になる（dismiss クリックは outside-click 経路が握り潰す ∴ 押した感触も hover も
  無いまま何も起きない、に見える）としてユーザ判断で修正。**当時の実装スケッチ「pop 各経路で
  last-cursor から `update_hover_in` 再評価。pop 時 main tree の layout は有効なので容易」は半分外れ**
  だった — pop と同じ drain で `rebuild_children` が走る経路（dropdown の項目選択でリストが並び替わる）
  があり、その新ノードには次の layout まで rect が無い。∴ 各 pop 経路ではなく **layout 後のフレーム
  1 箇所**に `WidgetTree::resync_hover` を置いた。FW-31 参照。
- 演習で炙った framework 実バグの**第5号**（G13/G14/G17/G19 に続く）。影響範囲は「クリックで開く
  menu/dropdown」trigger 全般。

### G16. 設定パネルの仕上がり差（Dropdown 寸法/角・MenuItem ラベル左寄せ）→ **解消（FW-20、2026-07-04 landed）**
- slice 3 で設定 dropdown を実機確認したユーザ指摘の「惜しい」点。いずれも忠実再現を妨げないが質感差。
  - **Dropdown が React の `<select>` より大きい** → **解消**: 真因は寸法非公開に加え、`measure` が
    content 高を `font+16` で返し **taffy がさらに vertical padding（2×8）を足す二重計上**で border box
    が `font+32`（≈48px）に膨れていたこと（React `px-2 py-1 text-sm` は ≈28px）。`Dropdown` に
    `padding_x`/`padding_y`/`min_height` builder を Input/Button と対称に公開し、measure の高さを
    **content = 1 行（`line_height`）／ 箱高 = `line_height + 2*padding_y`** に是正（doubling 解消）。
    `min_height` は border-box floor（padding を引いて content floor に変換）。既定 `padding_x=12`/
    `padding_y=8` で default も ≈`font+16` に締まる（旧 `font+32` から縮小。消費者は clone のみ）。
  - **Dropdown の角が四角い** → **解消**: トリガ枠を 4 本の sharp な `fill_rect` →
    `stroke_rect_rounded` 1 本に置換し `radius` 追従（FW-15 で角丸化した `Input`/`SecureInput` と対称）。
    `DropdownPopover` の枠も同様に丸めた。
  - **MenuItem のラベルがボックス左端に張り付く** → **解消**: `paint` が `text_x = layout.origin.x`
    で描き、宣言済み `padding_trbl(_,12,_,12)` の左 inset を無視していた。padding を `H_PADDING`/
    `V_PADDING` 定数に切り出して `style` と `paint` で共有し、`text_x = origin.x + H_PADDING`・shape 幅も
    `width - 2*H_PADDING` に。context menu / actions / 設定パネルのラベルが `px-3` 相当で inset される。
- **clone 配線**: `select()` に `padding_x(8).padding_y(4)`（`px-2 py-1`）で ≈25px に締め、設定行
  （`settings_select_row`/`settings_sort_row`）を `padding_xy(12, 8)` にして MenuItem の 12px inset と
  横位置を揃えた。テスト: dropdown_tests +4 / context_menu_tests +1。gate 全緑。**実機は
  knot_clone.exe が前セッションのインスタンスにロックされ再ビルド不可 → ユーザが閉じてから確認予定。**

### G17. Container の border が full-bleed な子背景に上書きされる → **framework 実バグ → 解消（FW-16、2026-07-02 landed）**
- 症状（実機、ユーザ指摘）: サイドバーに `border_right`（G10）を付けたのに**右端の線が見えない**。
  FW-15 で入れた4辺 `border()` も同じ穴を持つ（full-bleed な子があると消える）。
- 真因（端から端まで追跡）: `WidgetTree::paint_node`（`tree.rs:1148-1154`）は **親の `paint()` を子より
  先**に描く（`paint()` → children → `paint_post_children()`）。一方 `ScrollView::paint`
  （`scroll_view.rs:298`）は自分の layout 矩形**全体**を背景で塗る。サイドバー幅いっぱい（256px）の
  ノートリスト ScrollView が、先に描かれたサイドバーの右 border（x=255）を**上塗り**していた。旧実装の
  1px divider は**兄弟ノード**（全子孫の後に描画）だったので隠れなかった＝ FW-16 で box の辺に移した
  瞬間に踏んだ。
- 修正: Container の border 描画（4辺ストローク + 片側 divider）を `paint()` → **`paint_post_children()`**
  に移動。子の後に描かれる＝常に最前面になり、full-bleed な子背景でも隠れない。CSS の border が
  content box の外側に出て子に隠れないのと同じ挙動。塗り（background/hover）は `paint()` のまま。
- 影響範囲: 「full-bleed な子（ScrollView 等）を持つ枠付き Container」全般。app 作者が自然に踏む。
  回帰テスト2本（4辺 border / 片側 border が full-bleed 子の rect の**後**に emit される）。
- **G13 二重ガンマ・G14 layer anchor に続く「repro でしか炙れない framework 実バグ」第3号**。

### G18. drop-shadow / elevation プリミティブが無い（+ viewport 相対サイズ無し）→ **shadow 半分は FW-18 で解消（2026-07-03, 実機 OK）／ vh/vw 半分は FW-24 で解消（2026-07-07 landed）**
- 正解のモーダルは全て `shadow-xl`。scrim（`bg-black/50`）の上にカードを**浮かせる**のはこの影で、
  影が無いとカードが scrim に**べた張り**して立体感が消える（特にダーク背景で境界が溶ける）。
- sindon には shadow/elevation プリミティブが**一切無い**（`DrawRect` は fill + radius + border のみ、
  render にも box-shadow 相当なし）だった。∴ 発見時は 4モーダルとも影なしのフラットなカードで再現。
- **→ 解消（FW-18 shadow）**: 既存の角丸 SDF rect パイプラインに `DrawRect.blur` を1本足し、`blur>0` で
  クアッドを blur 分膨らませて `1 - smoothstep(0, blur, sdf)` でフェード（新パイプライン不要・`blur==0` は
  従来ビット等価）。`PaintContext::fill_shadow` → `Container::shadow(dx, dy, blur, spread, color)`（CSS
  `box-shadow` 準拠。背景の**背後**に描画＝はみ出た halo だけ見える）+ `Container::elevation(1..=4)` preset。
  clone は `card_column` の `elevation(4)`（=shadow-xl）で全モーダルに配線。**light/dark 両方でカードが
  scrim から浮くのを実機確認（2026-07-03）**。
- **→ 解消（FW-24 vh/vw、2026-07-07 landed）**: `FlexStyle` に viewport 相対長さの意図を別フィールドで
  保持し（`width_vw`/`height_vh`/`min_*_vw|vh`/`max_*_vw|vh` = 同軸のみ）、`resolve_viewport(vw, vh)` で
  Taffy の `length(px)` に焼き込む（Taffy 自体は vh/vw 単位を持たない）。ツリー側は各ノードに
  `has_viewport_dims` を持たせ、**add 時に現 viewport で解決 + リサイズ時に再解決**（既存の
  `refresh_visibility_styles` を `refresh_styles` に統合し、scroll-shrink / vh-vw 解決 / display:none を
  `effective_style` 1関数で一貫適用）。`Container` に `max_height_vh` 等を Input/Button と対称公開。
  clone の Restore カードを `max_height(576)` → `max_height_vh(80.0)` に置換 → ウィンドウ高の 80% に追従。
  layout +4 / widgets +4 テスト（resolve 値・リサイズ再解決・plain px 非追従の回帰ガード）。
  **`max-h-[80vh]` が本物の viewport 相対に**。**実機 OK（2026-07-07、ユーザ確認）**: ウィンドウを縦に潰すと
  カードが現ウィンドウ高の 80% に張り付いて縮み、ヘッダ + Cancel フッタは pinned・本文だけスクロール（720px
  決め打ちの旧実装なら短いウィンドウで画面外にはみ出していた）。

### G19. layer 内の clip がオフセット未適用 → **framework 実バグ第4号 → 解消（2026-07-03 landed）**
- 症状（実機、slice 4）: Restore モーダル（唯一 ScrollView を持つモーダル）で、**バックアップ行の中身が
  壊れる**。ファイル名（1行目・`truncate`）が消え、日付（2行目）だけ・しかも右が見切れ・Restore/Delete
  ボタンも消失。スクロール自体は正常。他3モーダル（ScrollView 無し）は無傷。
- 切り分け（レイアウトエンジンで実測、スクショ不可信のため）: card→ScrollView→行の rect は**全 path で
  完全に正しい**（root/9行 overflow/layer path すべて info=261・fname truncate・date 261・actions
  x=297–408）。∴ **レイアウトは無罪 = paint の問題**。
- 真因（paint 実測で確定）: `PaintContext::push_clip` が**現在の offset を畳み込んでいなかった**唯一の
  offset 系プリミティブ。`fill_rect`/`draw_glyph`/`push_rotation`/`set_ime_cursor_area` は全部
  「引数は widget ローカル座標 → active offset を足して絶対座標へ」なのに、`push_clip` だけ生の rect を
  そのまま積む。**main tree は offset=(0,0) なので無害**（∴ サイドバーの ScrollView は無事）だが、
  **中央寄せ layer は offset≈316px を子孫描画に適用する**（`tree.rs:1131` が layer subtree を
  `push_offset(layer_offset)` 内で描画）ため、ScrollView の clip は x=0（ローカル）のまま・グリフは
  x=332+（offset 適用済）で描かれ、**584 グリフ中 423 が自分の clip の外**に落ちて scissor で消える。
- 修正: `push_clip` で `current_offset()` を rect に畳み込む（`push_rotation` と対称）。
  landed 後の実測で clip が x=316/344（グリフ range 332–531 と一致）に移り、外れグリフ 423→56
  （残 56 は fold 下にスクロールアウトした行＝正しく空 clip）。
- 検証: unit test 2本（`push_clip_folds_in_the_active_offset` / offset 0 は不変）+ layer 統合 test 1本
  （中央 layer 内 ScrollView の子 rect の clip が viewport 座標に平行移動）。全 270+ widget test・
  workspace 緑・fmt/clippy 緑。**G13/G14/G17 に続く「repro でしか炙れない framework 実バグ」第4号**。
  G14（dispatch の layer-local rect）と同族の座標系バグだが、あちらは**イベント**、こちらは**paint clip**。

### slice 4 所見（Modal 群、2026-07-03）
モーダル4種を `LayerOptions::modal`（scrim + `ViewportCenter` + outside-click/Escape dismiss）で写経。
overlay の slice 3（`popover` プリセット）に対し、これは **`modal` プリセットの本命検証**。
- **✓ scrim + centered card は 1:1**。React の `fixed inset-0 bg-black/50 flex items-center
  justify-center` + centered card が `LayerOptions::modal` にそのまま乗る。overlay で温めた Layer 基盤が
  そのまま modal に効くことを確認（本 slice の主目的）。
- **✓ 多段レイヤースタックが素直に積める**。Backup Settings →（"Restore…"）→ Restore →（行の
  "Restore"/"Delete"）→ ConfirmDialog の **3段スタック**（scrim も3枚重畳）。`push_layer` は queue 式なので
  MenuItem の「設定 popover を pop → modal を push」も1ハンドラで順に drain。React の条件付きマウント
  （`{show && <Modal/>}`）と挙動一致。
- **✓ sticky header / scroll / footer**（Restore の `max-h-[80vh] flex flex-col` + `flex-1
  overflow-y-auto`）が、`max_height` 付き column + 中央の `grow` ScrollView + 上下 `border` セクションで
  成立。スクロール自体（固定ヘッダ/フッタ + 中央だけスクロール + バー）は実機一発 OK。
  **但し中身が壊れた → G19（framework 実バグ第4号）を発見・修正**（下記）。
- **✗→✓ shadow が無い → G18**（最大の質感差）→ **FW-18 で解消**（`Container::elevation(4)`、上記）。
- **踏んだ既知 gap**: G4（`px-6 py-4` 等の非対称 padding は uniform 近似）、G6（footer の `py-2` 高さ・
  `disabled:` スタイル不可、幅は `grow` で代替）、G12（`Input`/`SecureInput`/`Checkbox` の色が静的
  `Color` で theme 非追従 → 既定 chrome に委ね white/gray-800 の微差を許容）、**grid**（`grid-cols-2` は
  レイアウトプリミティブ無し → 2 grow 列で近似）。
- **未再現の状態**: ChangePassword の success サブ状態（中央の緑チェック）、`disabled:opacity-*` の
  無効化見た目、`ml-auto` は grow スペーサで代替。strength meter は静的に level 2（Fair）で表示。

### Button の hover/press 既定色（小・記録のみ）+ 透明ボタンの hover 文字色 → **解消（`hover_text_color`、2026-07-01 landed）**
- `Button::background(x)` だけ指定して `hover_background`/`press_background` を省くと、ホバー/押下で
  **既定の primary（青）にフェード**する（`button.rs`）。slice 3 でエラーバナーの透明 `×`
  が青い四角に化けたのはこれ（clone 側で両者を transparent 指定して解消）。バグではないが、
  「透明ボタン」を作る時に踏みやすい footgun。
- **訂正（2026-07-07）**: かつてここに「`hover_text_color` も無い（React の `hover:text-*` 不可）」と
  書いていたが**事実と逆**。透明ボタン（＝リンク）がホバーで**文字色だけ**濃くなる React の `hover:text-*`
  は、**`Button::hover_text_color` として既に landed 済み**（commit `35ac369`、2026-07-01 — 背景 hover と
  同じカーブで `text_color`→指定色にフェード、未指定なら不変）。この clone 自身が unlock「Forgot
  password?」/ recovery「Back」/ エラーバナー `×` / タグチップ `×` で**実使用**している。clone 演習が
  炙った小 framework 追加だが、G-gap 番号を振らず dogfood-log にも未記載で、台帳がコードに追いつけて
  いなかったドリフト（[[feedback-roadmap-hygiene]]）。dogfood-log にも一行追記した。

### slice 5 所見（Setup / Create Vault、2026-07-03）
`Auth/SetupScreen.tsx` を `examples/knot_clone/src/setup.rs` で写経。canonical 状態は
`docs/screenshots/setup.png`（有効な "Very strong" パスワード）。**framework 変更ゼロ・新規 gap
ゼロ** — Unlock（slice 1 以前）+ slice 4 で開けた語彙（`padding_x/min_height`・`border`・
`Checkbox`・強度メーター）だけで組めた。**実機 OK**（2026-07-03 ユーザ確認、light）。
- **✓ 構成は 1:1**: ヘッダ（text-4xl "Knot" + サブタイトル）/ 見出しブロック（"Create Vault"
  text-xl + 説明文 text-sm）/ `space-y-4` フィールド群（マスターPW欄 `px-4 py-3` + 強度メーター /
  確認欄 / リカバリーKey チェックボックス）/ 青フル幅 "Create Vault"。max-w-md 中央寄せも unlock と同じ
  `max_width(448).margin_x_auto()`。
- **強度メーターを静的再現**: React は入力から算出し空欄時は非表示だが、canonical に合わせ level 4
  （4 バー `bg-emerald-500` + ラベル "Very strong" `text-green-600/500`）を静的に描画。`emerald-500`
  を tokens に追加。メーター自体は「h-1 flex-1 rounded-full ×4 + text-xs ラベル」で slice 4 の
  `strength_meter` と同型（バーは grow+height(4)+radius(2)）。
- **エラーバナー非再現**: `validationError || error` は canonical が有効入力ゆえ出ない → 省略。chrome
  （`p-3 rounded-lg` 赤箱 + 1px border + text-sm）は既存 `border`/reactive bg で組める＝新 gap 無し。
  ソースにコメントで build 方法を明記。
- **踏んだ既知 gap（すべて既出・再確認のみ）**: G5（言語 select `absolute top-4 right-4` は
  absolute プリミティブ無しで省略、unlock 同様）/ G3・G6（入力・ボタンの寸法。FW-17 で入力側は解消済、
  Button `py-3` 高さは `radius`+align-stretch で近似）/ G12（`Checkbox` のチェック色が静的 Color →
  既定のテーマ primary に委ね、青チェックを得る）。
- **dev ナビ（clone ハーネス）**: 実アプリは vault 状態で分岐（loading → setup or unlock）し画面間
  リンクが無いので、レビュー用に `main.rs` へ dev shortcut を追加（**Ctrl+2=Setup / Ctrl+3=Unlock**、
  Loading/Recovery slice で Ctrl+1/4 を足す）。`ShortcutContext::event_ctx` 経由で `replace_screen`。
  Ctrl+D（light/dark）は従来どおり。

### アイコンについて（gap ではない・記録のみ）
- 正解は inline SVG アイコン。clone はアイコンフォント未同梱なので、ヘッダ操作・検索・ピン留め
  などは単一グリフ（⚙ ⋮ 🗑 🔒 🔍 📌）で近似した。アイコン描画の正式手段は FW-12（`App::font`
  + `family(Named)`）で既に解決済みであり、本 UI-only clone の対象外。レイアウト/枠/余白の
  gap 判定に影響しないため近似で進める。

### slice 6 所見（Recovery / Recover Vault、2026-07-03）
`Auth/RecoveryScreen.tsx` を `examples/knot_clone/src/recovery.rs` で写経。canonical は
**空の初期状態**（strength meter は `newPassword.length > 0`・error banner は `validationError ||
error` の条件付き ＝ 着地時は両方出ない）。**framework 変更ゼロ・新規 gap ゼロ** — Unlock/Setup +
slice 4 の語彙で組めた。**実機 OK**（2026-07-03 ユーザ確認、light/dark とも綺麗）。
- **✓ 構成は 1:1**: ヘッダ（"Knot" text-4xl + サブタイトル）/ 見出し（"Recover Vault" text-xl +
  説明 text-sm）/ `space-y-4` フィールド群（リカバリーキー textarea + 新PW + 確認PW）/ 青フル幅
  "Recover Vault" / 中央 "Back" テキストリンク。max-w-md 中央寄せ・space-y-6 は unlock/setup と同型。
- **★ このスライスの新語彙 = mnemonic の `<textarea>`（clone 初の複数行入力）**: `resize-none h-24`
  → `Input::multiline().padding_x(16).padding_y(12).height(96)`（当初 `min_height(96)` で近似 → FW-25 で
  `height(96)` に格上げ）。React ソースは**素の `<textarea>`（パスワード欄ではない・12単語は可視）**なので
  `SecureInput` でなく `Input`。border/radius は unlock 系と同じ既定 chrome。
- **小 gap 発見（G3 系に追記）= 固定ピクセル高の multiline が無い → 解消（FW-25、2026-07-08 landed）**:
  `h-24` は**固定96px でスクロール**。当初は `min_height(96)` で近似したが、`min_height` は**下限（floor）**
  ＝正確には *cap* でない。**実測すると Input は content サイズを measure 返さないので floor 96 にちょうど
  収まり静的には一致**する（メモの「sindon は箱が伸びる」は誤り）が、**floor ゆえ flex で stretch/grow
  される文脈では 96 を超えて膨らむ**（row の cross-axis stretch で実証。`min_height(96)`→300 に伸びる /
  `height(96)`→96 でキャップ）。∴ `h-24` の正しいプリミティブは *definite* な固定高。**解消**: `Input::height(px)`
  を追加（Taffy `size.height` を definite にセット・derived `min_height`/`height_full` を supersede）。
  multiline では definite ビューポート＝はみ出しを clip + 内部スクロール（既存の scroll 機構は
  `layout.size.height` ベース ∴ 流用）。single-line では箱を固定（テキストは縦中央）。従来の `height_full()`
  （＝*親*を埋める）に加えて**正確な固定箱**が手に入った。
- **Back リンク**は unlock の "Forgot password?" と同一 idiom（transparent fills + `hover_text_color`
  で文字色ダーク化）。**おまけ**: unlock の "Forgot password?" をダミーから実 Recovery 遷移に接続
  （本物の UnlockScreen と一致）。
- **踏んだ既知 gap（すべて既出・再確認のみ）**: G3/G6（入力・ボタンの寸法。入力側は FW-17 で解消、
  Button `py-3` は radius+align-stretch で近似）/ G12（`Input`/`SecureInput` の色が静的 Color →
  既定 chrome に委ねる）。strength meter・error banner は canonical で非表示ゆえ省略（chrome は
  setup.rs で既出、新 primitive 不要）。
- **dev ナビ**: Ctrl+4=Recovery を追加（Ctrl+1=Loading は残 slice で）。

### slice 7 所見（Loading、2026-07-03）
`App.tsx` のインライン `LoadingScreen` を `examples/knot_clone/src/loading.rs` で写経。アプリ最小の画面
＝ **framework 変更ゼロ・新規 gap ゼロ**。**実機 OK**（2026-07-03 ユーザ確認、light/dark とも綺麗）。
- **✓ 構成 1:1**: 両軸中央の text-center ブロックに "Knot" `text-4xl font-bold mb-4` + "Loading..."
  `text-gray-500`（`text-base`）。`background()` + 中央寄せ column の2 `TextWidget` だけで組めた。
- **注記**: 他画面と違い `p-4` **無し**、**スピナーも無し**（React 版がそもそも持たない）。∴ 新語彙・
  新 primitive ゼロ。dev ナビ Ctrl+1=Loading を追加（これで Ctrl+1〜4 が全画面に対応）。

---

## 全画面写経クローズ（2026-07-03）
Unlock / Setup / Recovery / Loading / Main（Sidebar・Editor・Overlays・Modals）の**全画面**を写経し切り、
それぞれ実機 OK。この演習（密な実画面 + ユーザの目）でしか炙れなかった **framework 実バグ4件**を発見・
修正した（**G13 二重ガンマ** [[fix-srgb-double-encode]] / **G14 layer 内 AnchorRect ズレ** / **G17
full-bleed 子が border を上書き** / **G19 push_clip の layer offset 未畳み込み** [[fix-push-clip-layer-offset]]）。
表現力 gap は **FW-15（border 系 G1/G2/G9）/ FW-16（整列 G11 + 片側 border G10）/ FW-17（入力寸法 G3）/
FW-18（drop-shadow G18 の shadow 半分）** として graduate 済み。

**FW-19（最小）で消化した gap**（2026-07-03、画面写経で出揃った残 gap の「app author が自然に踏む」3点）:
- **G6** Button の padding/高さ/固定幅(min_width)/disabled → **解消（FW-19）**
- **G12** Input の font-weight + chrome setter 非 Reactive → **解消（FW-19、Input のみ）**
- **G4（Container 側）** 非対称 padding（`padding_xy`/`padding_trbl`）→ **解消（FW-19）**

**なお未解決で持ち越す gap**:
- ~~**G16** Dropdown 寸法/角丸・MenuItem ラベル左寄せ（polish）~~ → **解消（FW-20、2026-07-04）**
- ~~**G5** absolute/固定エッジ anchor・右寄せ placement・click 時の trigger rect 取得~~ →
  **解消（FW-21、2026-07-04）** = `on_press_rect` + `AnchorRect{align}` + `Viewport{h,v,offset}`
- ~~**G18 の残り半分** viewport 相対サイズ vh/vw 無し~~ → **解消（FW-24、2026-07-07）** = `FlexStyle`
  の vh/vw 意図 + `resolve_viewport` + ツリーの add/resize 再解決。`Container::max_height_vh` 他。
  （shadow/elevation は **FW-18 で解消済**）
- ~~**G3 系の残り** 固定ピクセル高の multiline viewport 無し~~ → **解消（FW-25、2026-07-08）** =
  `Input::height(px)`（definite な固定箱・multiline は clip+スクロール。`min_height` は floor で cap でない
  問題を解消）。clone recovery textarea を `min_height(96)`→`height(96)` に格上げ。
- ~~**G7** focus が外側リング vs 正解は border 色変化（設計判断）~~ → **解消（FW-26、2026-07-08）** =
  テーマ `FocusStyle::indicator`（`Ring` 既定 / `Border`）。Border モードで border-bearing widget が
  focus 色に recolor、Button/Checkbox/borderless はリング fallback。
- ~~**G15** capturing layer の hover 固定~~ → **解消（FW-23、2026-07-06）** = interactive layer push 前に
  `clear_hover` で live chain へ `MouseLeave`（framework 実バグ第5号）
- **小・番号なし（2026-07-07 の台帳リコンサイルで明示化 — これまで G5 の `<details>` や slice 所見に
  埋没していて「持ち越し」として追えていなかった）** → **①② 解消（2026-07-12）／③ 棚上げ**:
  - ~~**`Container::min_width` 無し** — context menu `min-w-[140px]` を固定 `width(140)` で代用。~~ →
    **解消（2026-07-12）**: `Container::min_width(px)` / 対称で `min_height(px)` を追加（`FlexStyle::min_width`
    の薄いラッパ）。clone の context menu を `width(140)`→`min_width(140)` に置換（長い行で広がり 140 未満に
    ならない真の `min-w-*` 挙動に）。
  - ~~**`MenuItem` に disabled 状態が無い** — actions の "Export all notes"（`disabled:opacity-40`）が常時 enabled。~~ →
    **解消（2026-07-12）**: `MenuItem::disabled(impl Into<Reactive<bool>>)` を追加。Button と同じ
    `InteractionState` 規律（latching は塞ぐ／clearing は通す）で activation を握り、hover 抑止 + ラベルα半減
    （`disabled:opacity-40` 近似）。clone の "Export all notes" を React と同条件 `disabled(NOTES.is_empty())`
    に配線（ダミーは非空ゆえ enabled 表示＝React と一致）。テスト3本（never-fires / reactive gate / label dim）。
  - **grid レイアウトプリミティブ無し** — backup の `grid-cols-2` を 2×`grow(1.0)` 列で近似（modals.rs）。
    → **棚上げ（クローズではない）**。近似で見た目は成立しており、CSS grid を framework に入れる強い動機が
    今のところ clone のこの1箇所だけ ＝ ROI が薄い。番号（FW-）を振ると能動 backlog に昇格して着手圧が
    生まれるため、あえて振らず「実需が出たら再訪」として据え置く（[[feedback-roadmap-hygiene]]）。
    2×grow 近似は忠実再現をほぼ妨げないと slice 4 所見でも記録済み。

<!-- 以降、追加演習があれば追記 -->
