# Knot UI 完全再現 — gap ログ

**目的**: React/Tailwind 版 Knot (`knot-notes-app-v0.7.0-2026-04-27`) を「正解画面」とし、
shroud で見た目を写経する。写し取れない / 不格好になる箇所を gap として収集する。
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
| Setup | 未 |
| Recovery | 未 |
| Main — Sidebar shell | 完了（slice 1: パネル/ヘッダ/New Note/検索/ノート行） |
| Main — Editor pane | 完了（slice 2: タイトル欄/操作ボタン/タグ/ツールバー/本文/ステータスバー） |
| Main — Overlays (設定 dropdown / エラーバナー / context menu) | 完了（slice 3: 設定/⋮ dropdown・右クリック context menu・エラーバナー。**G5 検証台** — 下記） |
| 各種 Modal (Settings/Backup/ChangePw/Restore) | 未 |
| Loading | 未 |

---

## Gap 一覧

### G1. Container に border がない → **解消（FW-15、2026-06-29 landed）**
- 正解では border が遍在する: input (`border border-gray-300`)、エラー枠 (`border-red-300`)、
  カード、テーブルセル、サイドバー区切り、select。
- ~~shroud の `Container` は `background` + `radius` のみで枠線を描けない。~~
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

### G3. Input / SecureInput の padding・高さが非公開 — **中**
- 正解の入力欄は `px-4 py-3`（≒ 高さ 48px）。shroud の Input/SecureInput は内部 padding 固定で
  寸法を合わせられない。ボタンも同様（`py-3`）。
- 実装裏付け（`input.rs:1469`）: 単一行 Input は `FlexStyle::new().padding(8.0)
  .min_height(font_size + 20.0)` を**ハードコード**。`padding(8)` も `min_height`（font 14 →
  34px）も非公開。
- **実害例（Main slice 1 — 検索バーが「妙に縦長」）**: React は「枠付き div（`px-3 py-2`）+
  bare な borderless input」で ≈36px。shroud で同じ構成にすると「枠付きコンテナ padding 8 +
  borderless Input（自前 padding 8 + min_height 34）」= **≈50px** に膨らむ。`borderless()` は
  *枠*を消すだけで*内部 padding と min_height は残る*ため、カスタム chrome 行に Input を
  畳み込むと縦 padding が二重になる。
  - しかもこの1要素が G3 と **G4 を同時に踏む**: uniform padding なので「縦を詰める（コンテナ
    padding を下げる）」と「🔍 アイコンの左インセット」がトレードオフになり両立しない。
  - clone の暫定: コンテナ padding を 8→4 に下げて高さを ≈42px に寄せた（完全一致は不可）。
- **実害例 2（Main slice 2 — Editor 本文/タイトル）**: 本文 CodeMirror は `padding: 16px 24px`、
  タイトル欄は `text-2xl`（24px）の高い 1 行。shroud の Input は内部 padding 8px・min_height
  固定なので、本文の左右 24px インセットもタイトルの行高も合わせられない（`borderless()` でも
  内部 padding は残る）。clone は borderless + transparent で枠だけ消し、padding 差は許容。
- 候補対応: Input/SecureInput に `padding`(x/y) もしくは `min_height` を公開、または
  `borderless()` 時に内部 padding/min_height も落とす「chrome 無し」モード。Button も同様。

### G4. `padding` が上下左右一律のみ — **中**
- Tailwind は `px-4 py-3` / `px-2 py-1` のような非対称 padding が常用。
- `Container::padding(px)` は一律のみ。`padding_xy(x, y)` / 各辺 padding が欲しい。

### G5. absolute / コーナー配置のプリミティブがない — **中（slice 3 で検証完了 = 部分的に Layer で代替可）**
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

### G6. Button の padding / 高さ / 固定幅 / disabled スタイルが非公開 — **中**
- 正解の submit は `w-full py-3` かつ `disabled:bg-blue-800 disabled:cursor-not-allowed`。
- Button は `radius`/`background`/`text_color`/`hover_background`/`press_background`/`grow` は
  あるが padding・高さ・**固定幅**・disabled 状態スタイルがない。w-full は flex stretch で代替可。
- **実害例（Main slice 2 — ツールバーのボタン幅がバラつく）**: アイコンを単一グリフで近似して
  いるため advance 幅が `B`/`I`/`1.`/`[[`/絵文字で異なり、ボタンが不揃いに見える。正解は 18×18 の
  SVG を `p-2` で囲んだ均一な正方形。Button に `width`/`min_width` が無いので揃えられない（固定幅
  コンテナで包んでも Button 自体は内容 hug なので hover 背景が枠を埋めない）。根因の半分は
  アイコンフォント未同梱（FW-12、本 clone の対象外）だが、`Button::min_width` があれば均一化できる。

### G7. focus モデルの差（外側リング vs border 色変化）— **小〜中（要判断）**
- 正解は `focus:outline-none focus:border-blue-500`＝**枠線の色が変わるだけ**。
- shroud は外側に focus ring を描く（offset 付き）。見た目の質感が異なる。
- 「どちらが正か」は設計判断。Knot 再現の観点では border-color 方式が欲しい場面がある。

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
- ⚠ **訂正（2026-06-29）**: 当初ここで「shroud に色空間バグは無い」と結論したのは**勇み足**だった。
  HDR スクショが不可信で色判定を保留した隙に、**実在の二重ガンマ描画バグ（G13）を見逃していた**。
  G8（HDR キャプチャ）と G13（描画の二重ガンマ）は**別問題**。皮肉にも「色はユーザの目を正」と
  いう G8 の方法論があったから、HDR を切った実画面で G13 を炙り出せた。

### G9. 入力欄の枠が弱い（未フォーカスでほぼ枠なし）→ **解消（G1/G2 の派生）**
- ~~未フォーカスで枠がごく薄く、border/radius ビルダーが無いので常時枠を意図的に付けられない。~~
- G2 で `SecureInput` / `Input` とも border が既定 ON（`input_border` 追従）+ `radius`
  対応になったので、未フォーカスでも常時 `border border-gray-300 rounded-lg` 相当が出る。
- 残る差は G7（フォーカス時に「外側リング」が出る vs 正解は「枠線の色が変わるだけ」）。
  これは設計判断としてまだ open。常時枠が出るようになった分、G7 の体感差は小さくなった。

### G10. 片側 border (`border-r` / `border-b`) が引けない → **解消（FW-16、2026-07-02 landed）**
- 正解は仕切り線を片側 border で多用する: サイドバー右端 `border-r`、ヘッダ／検索／タグ各
  セクション下端 `border-b`、dropdown 内の区切り。
- ~~shroud の `Container::border(width, color)`（FW-15）は**4辺一括のみ**。1辺だけの線が引けない。~~
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
- ~~shroud は `center` / `justify_center`（主軸中央）/ `align_center`（交差軸中央）のみ。~~
- 対応: shroud ネイティブの `Justify`（Start/Center/End/SpaceBetween/SpaceAround/SpaceEvenly）
  / `Align`（Start/Center/End/Stretch）enum を `shroud_layout` に追加 →
  `FlexStyle::justify/align` + `Container::justify/align`。taffy を widget API に漏らさず
  `From` で内部マッピング（既存 center 系は温存）。clone のヘッダ `justify-between`・ステータス
  `text-right` は `grow(1.0)` スペーサ → `justify(SpaceBetween)`/`justify(End)` に置換。
  （ツールバーの `flex-1` スペーサは React も本物の spacer なので `grow` のまま。）

### G12. `Input` / `SecureInput` に font-weight（太字）が無い — **中（Main slice 2 で発見）**
- 正解の Editor タイトル欄は `text-2xl font-bold`。`TextWidget` には `weight(FontWeight)` が
  あるが、`Input` には `font_size` しか無く太字にできない。
- 暫定対応（slice 2）: タイトル欄は `font_size(24)` のみ。太字は再現せず。
- 候補対応: `Input::weight(FontWeight)`（描画は既に shape_rich が weight 対応済なので配線のみ）。
- 補足（小・派生）: `Input::background` / `border_color` / `text_color` は **`Color`** を取り
  `Reactive<Color>` ではない（Container/Button は `Reactive` 受け）。明示指定すると live
  テーマ切替に追従しない。clone は既定（`on_surface`/`input_border` 追従）に委ねて回避。

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

### G15. capturing layer が trigger の `MouseLeave` を食い、ホバーが固定される → **framework gap（真因特定・棚上げ：実害軽微）**
- 症状（slice 3、実機 + ユーザ指摘）: クリックで popover を開く trigger（`hoverable` な
  `Container`、gear/⋮ 等）が、popover を閉じた後も**ホバー色（灰）のまま固定**。もう一度ホバーし
  直すと直る（実害は軽微だが「全ボタンをホバー状態で固定できてしまう」違和感）。
- **第一次仮説（外れ）**: 「layer push 時に main tree の現ホバーを `clear_hover` で消せばよい」。実装して
  診断ログを仕込んだところ、**push 時点で `self.hovered` は毎回 `None`** だった → `clear_hover` は完全に
  no-op。撤去済み。
- **真因（診断で判明）**: gear の `hover_anim` は 1.0（灰色）なのに、それを hover として追跡している
  ノードが無い（`hovered=None`）＝**hover の「見た目（anim）」と「状態（`self.hovered`）」が layer 境界を
  またいで desync**。gear に来るべき `MouseEnter` の対の `MouseLeave` が layer 遷移のどこかで迷子になり、
  `hover_anim` が孤児化して 1.0 に張り付く。enter/leave は `self.hovered` の遷移に紐づくので、状態が
  既に None だと二度と leave が出ない。
- **素直な修正が効かない二重の壁**: (a) push 時は `hovered=None` なので直接消せない、(b) 「push/pop 時に
  カーソル位置から hover 再評価」も、**drain 時点では新 layer のレイアウトが未計算**（`layer.offset` は
  次フレームの `compute_layout` まで stale/0）なので hit-test を当てられない。
- **正攻法（要・別途設計）**: WidgetTree に最終カーソル位置を保持し、レイアウト確定後（次フレーム冒頭 or
  paint 前）に layer push/pop を検知して hover を再同期する。あるいは enter/leave を `hover_anim` を持つ
  全 Container に対し layer 遷移でフラッシュする。いずれも hover/layer 相互作用に踏み込むので test 必須。
- **判断**: ユーザ評価どおり**実害軽微**につき棚上げ（真因はここに記録）。FW-13 tooltip の click-through
  設計（`layer.rs:96-104`、非 interactive は `MouseLeave` を食わない）の「interactive 版の取りこぼし」に
  当たる。影響範囲は「クリックで開く menu/dropdown」trigger 全般。

### G16. 設定パネルの仕上がり差（Dropdown 寸法/角・MenuItem ラベル左寄せ）— **小（polish・要判断）**
- slice 3 で設定 dropdown を実機確認したユーザ指摘の「惜しい」点。いずれも忠実再現を妨げないが質感差。
  - **Dropdown が React の `<select>` より大きい**: `Dropdown` は `style` で `padding_trbl(8,12,8,12)` +
    `measure` で高さ `font+16` を持つ（`dropdown.rs:220/258`）→ ≈46px。React は `px-2 py-1 text-sm`
    ≈28px。padding/高さの builder が無く詰められない（G3/G6 の「寸法非公開」が **Dropdown にも及ぶ**）。
  - **Dropdown の角が四角い**: トリガ枠を `stroke_rect_rounded` ではなく 4 本の sharp な `fill_rect`
    で描く（`dropdown.rs:294-309`、コメントも "sharp corners are fine"）。fill は `radius` 角丸でも
    枠は直角 → FW-15 で角丸化した `Input`/`SecureInput` と非対称。`Dropdown` 枠も radius 追従にしたい。
  - **MenuItem のラベルがボックス左端に張り付く**: `menu_item.rs:129` がテキストを `layout.origin.x`
    （= ボックス左端）に描く。宣言した `padding_trbl(_,12,_,12)` は箱を広げるだけで**ラベルを inset
    しない**ので、行は左端に寄る。設定パネルの「Backup Settings / Change Master Password」が、上の
    select 行ラベル（コンテナ padding 8px ぶん inset）より左に出て不揃いに見えるのはこれ。React は
    全行 `px-3` で揃う。修正案: MenuItem paint で left-padding ぶん inset、または clone 側でパネルに
    水平 padding を与え行を揃える。
- いずれも FW-17 系（寸法公開）の候補に合流可（FW-16 は整列+片側 border のみに絞った）。
  clone は当面そのまま（動作は正しい）。

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

### Button の hover/press 既定色（小・記録のみ）
- `Button::background(x)` だけ指定して `hover_background`/`press_background` を省くと、ホバー/押下で
  **既定の primary（青）にフェード**する（`button.rs:310-319`）。slice 3 でエラーバナーの透明 `×`
  が青い四角に化けたのはこれ（clone 側で両者を transparent 指定して解消）。バグではないが、
  「透明ボタン」を作る時に踏みやすい footgun。`hover_text_color` も無い（React の `hover:text-*` 不可）。

### アイコンについて（gap ではない・記録のみ）
- 正解は inline SVG アイコン。clone はアイコンフォント未同梱なので、ヘッダ操作・検索・ピン留め
  などは単一グリフ（⚙ ⋮ 🗑 🔒 🔍 📌）で近似した。アイコン描画の正式手段は FW-12（`App::font`
  + `family(Named)`）で既に解決済みであり、本 UI-only clone の対象外。レイアウト/枠/余白の
  gap 判定に影響しないため近似で進める。

<!-- 以降、画面を進めながら追記 -->
