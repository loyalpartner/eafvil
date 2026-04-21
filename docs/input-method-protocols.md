# 输入法协议时序参考

本文档只讲协议本身，不涉及任何具体合成器实现。

## 目录

1. [text_input_v3（编辑器侧，Wayland 客户端收 preedit / commit）](#1-text_input_v3编辑器侧wayland-客户端收-preedit--commit)
2. [input_method_v2（IME 侧，IME 作为 Wayland 特权客户端）](#2-input_method_v2ime-侧ime-作为-wayland-特权客户端)
3. [zwp_virtual_keyboard_v1（IME 注入按键 / keymap）](#3-zwp_virtual_keyboard_v1ime-注入按键--keymap)
4. [Wayland IME 端到端交互时序（三方串起来）](#4-wayland-ime-端到端交互时序三方串起来)
5. [X11 XIM（Xlib / Xt 经典输入法协议）](#5-x11-ximxlib--xt-经典输入法协议)
6. [X11 XIM 端到端交互时序](#6-x11-xim-端到端交互时序)
7. [IBus / fcitx5 D-Bus 旁路通道](#7-ibus--fcitx5-d-bus-旁路通道)
8. [XWayland 输入法桥](#8-xwayland-输入法桥)
9. [参考协议链接](#参考协议链接)

## 角色术语

- **编辑器 / Text app**：持有输入焦点、想接收 preedit 的客户端
- **IME / Input method**：输入法前端进程（fcitx5、ibus、xim daemon 等）
- **Compositor / X server**：中介，广播焦点、转发 commit / preedit、路由按键

---

## 1. text_input_v3（编辑器侧，Wayland 客户端收 preedit / commit）

核心对象：
- `zwp_text_input_manager_v3`（全局 singleton）
- `zwp_text_input_v3`（每客户端 + 每 seat 一个；编辑器持有）

协议是**焦点门控**的：客户端只有在自己的 `wl_surface` 获得键盘焦点且自己 `enable` 过 text_input 时，才会进入 "active" 状态并收到 preedit / commit。

### 1.1 启用与输入状态同步

```
Editor client                              Compositor
      │                                          │
      │ zwp_text_input_v3.enable                 │
      ├─────────────────────────────────────────►│
      │ set_content_type(hint, purpose)          │
      ├─────────────────────────────────────────►│
      │ set_cursor_rectangle(x, y, w, h)         │
      ├─────────────────────────────────────────►│
      │ set_surrounding_text(text, cursor, anchor)
      ├─────────────────────────────────────────►│
      │ set_text_change_cause(input_method|other)│
      ├─────────────────────────────────────────►│
      │ commit                                   │ ← 把以上 pending state 原子
      ├─────────────────────────────────────────►│   落到 "current" state
      │                                          │
      │ (enter 事件在 wl_keyboard.enter 同 surface 到来时发)
      │ zwp_text_input_v3.enter(surface)         │
      │◄─────────────────────────────────────────┤
```

关键点：
- `enable` 本身不生效，**必须** `commit` — 协议用 double-buffered state
- `commit` 之后每次 `done` 事件会带一个 serial，编辑器用这个 serial 对齐自身编辑状态
- `set_cursor_rectangle` 是 surface-local，**合成器**负责加窗口位置把 IME 弹窗挪到实际光标位置
- 失去焦点时合成器发 `leave`；编辑器可再 `enable/commit` 重新进入

### 1.2 收 preedit 与 commit

```
Editor client                              Compositor / IME
      │                                          │
      │ zwp_text_input_v3.preedit_string(        │
      │   text, cursor_begin, cursor_end)        │
      │◄─────────────────────────────────────────┤
      │ zwp_text_input_v3.delete_surrounding_text│
      │   (before_length, after_length)          │
      │◄─────────────────────────────────────────┤
      │ zwp_text_input_v3.commit_string(text)    │
      │◄─────────────────────────────────────────┤
      │ zwp_text_input_v3.done(serial)           │ ← 一组事件的原子 flush
      │◄─────────────────────────────────────────┤
      │                                          │
      │ 编辑器按 done 的 serial 提交:            │
      │   1. 先删 surrounding 要求的长度         │
      │   2. 插入 commit_string                  │
      │   3. 把 preedit_string 作为高亮文本      │
      │      （仍在 composition 中、未 commit）  │
```

`preedit_string(None, ...)` 表示清掉当前 preedit。`cursor_begin == cursor_end == -1` 表示隐藏 preedit 光标。

### 1.3 生命周期

```
Editor client                              Compositor
      │                                          │
      │ disable                                  │
      ├─────────────────────────────────────────►│
      │ commit                                   │
      ├─────────────────────────────────────────►│
      │                                          │ leave(surface) (若之前是 enter 状态)
      │◄─────────────────────────────────────────┤
      │ zwp_text_input_v3.destroy                │
      ├─────────────────────────────────────────►│
```

---

## 2. input_method_v2（IME 侧，IME 作为 Wayland 特权客户端）

核心对象：
- `zwp_input_method_manager_v2`
- `zwp_input_method_v2`（每 seat 一个；只能有一个 IME 同时绑定）
- `zwp_input_popup_surface_v2`（IME 弹窗；挂在 input_method 上，不是普通 xdg_surface）
- `zwp_input_method_keyboard_grab_v2`（可选；IME 抓整条键盘）

### 2.1 IME 启动与激活状态

```
IME client                                 Compositor
    │                                           │
    │ manager.get_input_method(seat)            │
    ├──────────────────────────────────────────►│
    │                                           │
    │ 某客户端 enable 了 text_input_v3 且持焦点│
    │                                           │
    │ zwp_input_method_v2.activate              │
    │◄──────────────────────────────────────────┤
    │ zwp_input_method_v2.surrounding_text(     │
    │   text, cursor, anchor)                   │
    │◄──────────────────────────────────────────┤
    │ zwp_input_method_v2.text_change_cause(c)  │
    │◄──────────────────────────────────────────┤
    │ zwp_input_method_v2.content_type(h, p)    │
    │◄──────────────────────────────────────────┤
    │ zwp_input_method_v2.done                  │ ← 原子 flush
    │◄──────────────────────────────────────────┤
```

协议要求合成器按每次 text_input 客户端的 `commit` 重发全套 activate + state + done。换 focus / 换 text_input 客户端 → 先 `deactivate` → 新一轮 `activate` + state + done。

### 2.2 IME 产出文字（对称于 §1.2）

```
IME client                                 Compositor                Editor client
    │                                           │                          │
    │ set_preedit_string(text, begin, end)      │                          │
    ├──────────────────────────────────────────►│                          │
    │ delete_surrounding_text(before, after)    │                          │
    ├──────────────────────────────────────────►│                          │
    │ commit_string(text)                       │                          │
    ├──────────────────────────────────────────►│                          │
    │ commit(serial)                            │ ← serial 必须和最后一次 │
    ├──────────────────────────────────────────►│   done 的 serial 对齐    │
    │                                           │                          │
    │                                           │ zwp_text_input_v3.       │
    │                                           │   preedit_string(...)    │
    │                                           ├─────────────────────────►│
    │                                           │   delete_surrounding_... │
    │                                           ├─────────────────────────►│
    │                                           │   commit_string(...)     │
    │                                           ├─────────────────────────►│
    │                                           │   done(serial)           │
    │                                           ├─────────────────────────►│
```

合成器不做内容解释，纯翻译：IME 的 `commit_string` 一字一字穿到 text_input 的 `commit_string`。

### 2.3 IME 弹窗

```
IME client                                 Compositor
    │                                           │
    │ get_input_popup_surface(wl_surface)       │
    ├──────────────────────────────────────────►│
    │                                           │
    │ (当 text_input 有 set_cursor_rectangle)   │
    │ popup.text_input_rectangle(x, y, w, h)    │ ← surface-local on the
    │◄──────────────────────────────────────────┤   text_input surface
    │                                           │
```

IME 据此把候选窗对齐到文字光标附近。合成器负责弹窗位置策略，但**位置信息**只来自 popup.text_input_rectangle 事件 + wl_surface 自身 commit 的 buffer。

### 2.4 键盘抓取（让 IME 决定按键是否转发）

```
IME client                                 Compositor
    │                                           │
    │ input_method.grab_keyboard                │
    ├──────────────────────────────────────────►│
    │                                           │
    │ grab.keymap(fd, size)                     │
    │◄──────────────────────────────────────────┤
    │ grab.key(serial, time, key, state)        │ ← 原本给编辑器的按键
    │◄──────────────────────────────────────────┤   转发到 IME
    │ grab.modifiers(...)                       │
    │◄──────────────────────────────────────────┤
    │                                           │
    │ (IME 决定:                                │
    │    字母键吃掉 → set_preedit_string        │
    │    其他键透传 → 虚拟键盘注入 §3)          │
```

有抓取期间，原本走 `wl_keyboard.key` 到编辑器的按键改走 `grab.key` 给 IME。IME 想让编辑器直接收原始按键，要另外通过 §3 `zwp_virtual_keyboard_v1.key` 注入。

---

## 3. zwp_virtual_keyboard_v1（IME 注入按键 / keymap）

通常由 IME 和 input_method_v2 搭配使用：IME 抓了键盘（§2.4），又想原样转发某些按键，或者发热键把候选翻页同时不污染编辑器的文字流。

核心对象：
- `zwp_virtual_keyboard_manager_v1`
- `zwp_virtual_keyboard_v1`（每 seat 一个）

```
Virtual keyboard client                    Compositor
    │                                           │
    │ manager.create_virtual_keyboard(seat)     │
    ├──────────────────────────────────────────►│
    │ virtual_keyboard.keymap(format, fd, size) │ ← 告诉合成器 layout
    ├──────────────────────────────────────────►│   (一般 xkb v1)
    │                                           │
    │ virtual_keyboard.modifiers(               │
    │   mods_depressed, mods_latched,           │
    │   mods_locked, group)                     │
    ├──────────────────────────────────────────►│
    │ virtual_keyboard.key(time, key, state)    │
    ├──────────────────────────────────────────►│
    │                                           │ wl_keyboard.key
    │                                           ├───────────────────► Focus client
```

`key` 的 keycode 是**硬件 level**（evdev + 8），合成器用自己的 xkb context + virtual_keyboard 提供的 keymap 翻译成 keysym 再发下去。

---

## 4. Wayland IME 端到端交互时序（三方串起来）

§1–§3 分别讲了三个协议对象；实际工作时三方（**编辑器**、**合成器**、**IME**）必须配合。本节把 "用户激活输入法、按键、产出汉字" 的完整路径画透。

### 4.1 连接与对象建立（静态）

在任何人按键之前：

```
Editor client                  Compositor                         IME client
      │                             │                                 │
      │ wl_registry.bind            │                                 │
      │   (zwp_text_input_manager_v3)│                                │
      ├────────────────────────────►│                                 │
      │                             │◄────────────────────────────────┤ bind input_method_manager_v2
      │                             │◄────────────────────────────────┤ bind virtual_keyboard_manager_v1
      │                             │                                 │
      │ manager.get_text_input(seat)│                                 │
      ├────────────────────────────►│                                 │
      │ ← zwp_text_input_v3         │                                 │
      │                             │ manager.get_input_method(seat)  │
      │                             │◄────────────────────────────────┤
      │                             │ → zwp_input_method_v2           │
      │                             │   (seat 已有 IME 则发 unavailable)
      │                             │                                 │
      │                             │ virtual_keyboard.keymap(fd,len) │
      │                             │◄────────────────────────────────┤
```

关键约束：
- 每个 seat 只能绑定**一个** input_method_v2；重复 bind 第二个会立刻收到 `unavailable` 事件并被禁用。这是为什么系统里同时跑两个 IME（ibus + fcitx5）会有一个不干活
- `virtual_keyboard.keymap` 在 IME 启动时就要发；后面真正要注入按键时合成器才有 layout 可用
- 编辑器创建 `zwp_text_input_v3` 是**惰性**的 — 没聚焦、没 enable 时合成器什么事件都不往它身上送

### 4.2 用户把光标移进编辑框：激活输入会话

"用户激活输入法" 在 Wayland 世界里分两步：

1. **合成器激活 text_input**（wl_keyboard.enter → text_input.enter）— 总是发生，只要聚焦的客户端创建过 `zwp_text_input_v3`
2. **编辑器激活 IME 会话**（`enable` + `commit`）— 只在编辑器确实想要 IME 输入时才发（点中文本框、进入编辑模式）

```
Editor client                 Compositor                         IME client
      │                            │                                 │
      │ 用户点编辑器的文本框、Alt-Tab 切过来 …                       │
      │                            │                                 │
      │ wl_keyboard.enter(surface, keys)                             │
      │◄───────────────────────────┤                                 │
      │ zwp_text_input_v3.enter(surface)                             │
      │◄───────────────────────────┤                                 │
      │                            │                                 │
      │ (此时编辑器还没 enable，合成器不往 IME 发 activate — IME 静默)│
      │                            │                                 │
      │ 编辑器 UI 决定"这是个文本框，需要输入法":                    │
      │                            │                                 │
      │ zwp_text_input_v3.enable   │                                 │
      ├───────────────────────────►│                                 │
      │ set_surrounding_text(      │                                 │
      │   text="前已有文|此处光标",│                                 │
      │   cursor=18, anchor=18)    │                                 │
      ├───────────────────────────►│                                 │
      │ set_content_type(          │                                 │
      │   hint=SPELLCHECK,         │                                 │
      │   purpose=NORMAL)          │                                 │
      ├───────────────────────────►│                                 │
      │ set_cursor_rectangle(      │                                 │
      │   x=120, y=80, w=2, h=20)  │                                 │
      ├───────────────────────────►│                                 │
      │ commit                     │ ← 所有 pending 原子落盘         │
      ├───────────────────────────►│                                 │
      │                            │                                 │
      │                            │ zwp_input_method_v2.activate    │
      │                            ├────────────────────────────────►│
      │                            │ surrounding_text("…", 18, 18)   │
      │                            ├────────────────────────────────►│
      │                            │ content_type(SPELLCHECK,NORMAL) │
      │                            ├────────────────────────────────►│
      │                            │ text_change_cause(INPUT_METHOD) │
      │                            ├────────────────────────────────►│
      │                            │ done                            │ ← IME 的 batch 终止
      │                            ├────────────────────────────────►│
      │                            │                                 │
      │                            │ (IME 内部: 挂候选窗、切到中文态)
```

注意：
- 在 text_input 还没 `enable+commit` 之前，即便 wl_keyboard.enter 过来了，IME 也不会被 `activate` — IME 完全不知道有新焦点
- 一次编辑器 `commit` 对应合成器 "activate + surrounding + content + done" 一整批。这批是 IME 能看到的 "输入场景换了"
- 如果用户点到同一个文本框的不同位置，编辑器通常再发一次 `set_cursor_rectangle + set_surrounding_text + commit`；合成器这时**不**重发 activate，只发新 state + done

### 4.3 用户按下 "n"：合成器 → IME → 编辑器的完整一拍

假设 IME 尚未 `grab_keyboard`（典型的中文拼音 IME 默认不 grab，按键先到合成器 hit-test）：

#### 4.3a 不带 grab：编辑器同时收键与 preedit

```
Host kbd          Compositor            Editor              IME client
    │                 │                    │                    │
    │ key 'n' down    │                    │                    │
    ├────────────────►│                    │                    │
    │                 │ wl_keyboard.key(   │                    │
    │                 │   'n', pressed)    │                    │
    │                 ├───────────────────►│                    │
    │                 │                    │                    │
    │                 │ (编辑器 toolkit:   │                    │
    │                 │  看到有 active text_input → 不自己处理 │
    │                 │  而是等 IME 事件)                      │
    │                 │                    │                    │
```

咦 — 这里问题大了：合成器直接把 `wl_keyboard.key` 发到编辑器，IME 根本没看到 'n'。怎么让 IME 处理？答案：**要让 IME 吃到按键，它必须先 grab**。不 grab 的后果是按键直接进编辑器，IME 只是个被动 "展示候选" 的角色 — 这是 IME 设计者自己的选择（比如有些一拼法让用户用 Shift 翻页、IME 只从 `surrounding_text` 里读整行重新识别）。

绝大多数中文 IME 会在 activate 后立刻 grab：

#### 4.3b 带 grab：IME 是按键第一拿手（常见路径）

```
Host kbd          Compositor             IME client              Editor
    │                 │                       │                    │
    │ （IME 在收到 activate 后立刻:）         │                    │
    │                 │ input_method.          │                    │
    │                 │   grab_keyboard        │                    │
    │                 │◄───────────────────────┤                    │
    │                 │ grab.keymap(fd, size)  │                    │
    │                 ├───────────────────────►│                    │
    │                 │                        │                    │
    │ key 'n' down    │                        │                    │
    ├────────────────►│                        │                    │
    │                 │ （grab 生效，按键改走 grab，              │
    │                 │   wl_keyboard.key 不发给编辑器）          │
    │                 │                        │                    │
    │                 │ grab.modifiers(...)    │                    │
    │                 ├───────────────────────►│                    │
    │                 │ grab.key(serial, time, │                    │
    │                 │         'n', pressed)  │                    │
    │                 ├───────────────────────►│                    │
    │                 │                        │                    │
    │                 │ IME 内部:              │                    │
    │                 │  - 读 xkb 翻出 keysym 'n'                  │
    │                 │  - 接到拼音缓冲: "n"   │                    │
    │                 │  - 查候选: "你 呢 哪 内 拿 …"              │
    │                 │  - 挂候选窗                               │
    │                 │                        │                    │
    │                 │ set_preedit_string(    │                    │
    │                 │   "n", 0, 1)           │                    │
    │                 │◄───────────────────────┤                    │
    │                 │ commit(serial=S)       │ ← 和最新一次收到   │
    │                 │◄───────────────────────┤   的 done.serial  │
    │                 │                        │   对齐             │
    │                 │                        │                    │
    │                 │ zwp_text_input_v3.     │                    │
    │                 │   preedit_string(      │                    │
    │                 │     "n", 0, 1)         │                    │
    │                 ├────────────────────────┼───────────────────►│
    │                 │ done(serial=S)         │                    │
    │                 ├────────────────────────┼───────────────────►│
    │                 │                        │                    │
    │                 │ (编辑器在原光标位置画下划线"n")           │
    │                 │                        │                    │
```

关键事实：
- **grab 开着的时候按键既不发 `wl_keyboard.key` 也不发 `grab.key` 给编辑器**。编辑器根本看不到用户按了啥，只能等 IME 通过 preedit/commit 告诉它
- `preedit_string` 里的两个 int 是 "光标应该显示在 preedit 的哪一段" 的字节偏移 — `(0, 1)` 意思是从字节 0 到字节 1 选中／加粗；编辑器据此画候选词高亮
- IME → 合成器 → 编辑器这一跳**没有独立 ack**；编辑器收到 `done(serial)` 就必须在下一帧前把状态渲染出来

#### 4.3c 用户继续按 "i"：preedit 追加

```
Host kbd          Compositor             IME client              Editor
    │                 │                       │                    │
    │ key 'i' down    │                        │                   │
    ├────────────────►│ grab.key('i', pressed)│                   │
    │                 ├───────────────────────►│                   │
    │                 │                        │ 拼音 buf: "ni"   │
    │                 │                        │ 候选: "你 尼 泥 …"│
    │                 │ set_preedit_string(    │                   │
    │                 │   "ni", 0, 2)          │                   │
    │                 │◄───────────────────────┤                   │
    │                 │ commit(serial=S+1)     │                   │
    │                 │◄───────────────────────┤                   │
    │                 │ preedit_string("ni",   │                   │
    │                 │                0, 2)   │                   │
    │                 ├───────────────────────────────────────────►│
    │                 │ done(S+1)              │                   │
    │                 ├───────────────────────────────────────────►│
```

编辑器下划线扩成 "ni"，原位置 caret 仍在编辑器那边（光标属于编辑器的 "真状态"；IME 画的是 preedit，两者独立）。

### 4.4 用户按空格确认候选 "你"：commit 一拍

```
Host kbd          Compositor             IME client              Editor
    │                 │                        │                   │
    │ key Space down  │ grab.key(Space, pressed)│                  │
    ├────────────────►├───────────────────────►│                   │
    │                 │                        │                   │
    │                 │                        │ IME: 用户要第 1 个│
    │                 │                        │   候选 "你"       │
    │                 │                        │ 关闭候选窗        │
    │                 │                        │                   │
    │                 │ set_preedit_string(    │                   │
    │                 │   None, 0, 0)          │ ← 先清掉 preedit  │
    │                 │◄───────────────────────┤                   │
    │                 │ commit_string("你")    │                   │
    │                 │◄───────────────────────┤                   │
    │                 │ commit(serial=S+2)     │                   │
    │                 │◄───────────────────────┤                   │
    │                 │                        │                   │
    │                 │ preedit_string(        │                   │
    │                 │   None, 0, 0)          │                   │
    │                 ├───────────────────────────────────────────►│
    │                 │ commit_string("你")    │                   │
    │                 ├───────────────────────────────────────────►│
    │                 │ done(S+2)              │                   │
    │                 ├───────────────────────────────────────────►│
    │                 │                        │                   │
    │                 │ (编辑器在原光标插 "你"，光标右移 1 字符)   │
    │                 │                        │                   │
    │ 编辑器随后主动发 set_surrounding_text 把新内容同步给合成器: │
    │                 │                        │                   │
    │                 │ set_surrounding_text(  │                   │
    │                 │   "前已有文你|", 21,   │                   │
    │                 │   21)                  │                   │
    │                 │◄───────────────────────────────────────────┤
    │                 │ set_cursor_rectangle( │                    │
    │                 │   x=138, y=80, …)     │                    │
    │                 │◄───────────────────────────────────────────┤
    │                 │ commit                │                    │
    │                 │◄───────────────────────────────────────────┤
    │                 │                        │                   │
    │                 │ input_method.          │                   │
    │                 │   surrounding_text(    │                   │
    │                 │   "前已有文你|", 21,21)│                   │
    │                 ├───────────────────────►│                   │
    │                 │ text_change_cause(     │                   │
    │                 │   INPUT_METHOD)        │                   │
    │                 ├───────────────────────►│                   │
    │                 │ done                   │                   │
    │                 ├───────────────────────►│                   │
```

这里的几条强约束：
- IME 发的 **"一批" = set_preedit + delete_surrounding + commit_string + commit(serial)**；合成器保证这组要么全送达，要么一个都不送
- 编辑器收到后**先删 surrounding（若 delete_surrounding_text 存在）再插 commit_string 再画 preedit**，顺序反了光标位置会错
- commit_string 和 preedit_string 可以在**同一批**里同时存在：意思是 "把这个字固化、同时画上新的 preedit"（比如五笔输入多字连打）
- 编辑器插字后要反向发 `set_surrounding_text + set_cursor_rectangle + commit` 让 IME 知道新光标在哪儿 — 候选窗要跟过去贴到新光标；这就是 **双向 state 同步**

### 4.5 光标追踪：candidate popup 的位置更新回路

假设 IME 用 `get_input_popup_surface` 开了个候选窗（而不是自己做 layer-shell 弹窗）：

```
Editor             Compositor                  IME
   │                    │                       │
   │ set_cursor_rectangle(120, 80, 2, 20)       │
   ├───────────────────►│                       │
   │ commit             │                       │
   ├───────────────────►│                       │
   │                    │                       │
   │                    │ input_popup_surface.  │
   │                    │   text_input_rectangle│
   │                    │   (120, 80, 2, 20)    │ ← surface-local on
   │                    ├──────────────────────►│   the text_input surface
   │                    │                       │
   │                    │ (IME 根据这个矩形算候选窗应该
   │                    │  贴在矩形左下 or 右上，然后
   │                    │  wl_surface.commit 候选窗 buffer)
   │                    │                       │
   │                    │ popup_surface 的 wl_surface.commit
   │                    │◄──────────────────────┤
   │                    │                       │
   │                    │ (合成器把候选窗放在文本 rectangle
   │                    │  下方，屏外时翻到上方)
```

关键：`set_cursor_rectangle` 的坐标系是**编辑器自己的 wl_surface 内部**（surface-local）；合成器负责加上窗口在屏幕上的位置把候选窗放对地方。IME 拿到的矩形也是 surface-local，不是屏幕绝对坐标 — 因为 IME 可能压根不知道编辑器在哪儿。

### 4.6 用户按 ESC 取消组字

```
Compositor            IME                                 Editor
    │                  │                                   │
    │ grab.key(Esc)    │                                   │
    ├─────────────────►│                                   │
    │                  │ IME: 清拼音缓冲, 关候选窗         │
    │                  │                                   │
    │ set_preedit_string(None, 0, 0)                       │
    │◄─────────────────┤                                   │
    │ commit(serial)   │                                   │
    │◄─────────────────┤                                   │
    │                  │                                   │
    │ preedit_string(None, 0, 0)                           │
    ├──────────────────────────────────────────────────────►│
    │ done(serial)     │                                   │
    ├──────────────────────────────────────────────────────►│
```

IME **不**发 `commit_string` — 整次组字白费，编辑器光标不动。Esc 按键本身也不会透给编辑器（grab 期间按键只给 IME）。

要让 Esc 既取消组字又被编辑器处理（比如 Vim 要退出 insert mode），IME 要显式用 `virtual_keyboard.key` 再"注射"一次 Esc：

```
Compositor            IME
    │                  │
    │ grab.key(Esc)    │
    ├─────────────────►│
    │                  │
    │ IME 无拼音 buf → 透传逻辑:
    │                  │
    │ virtual_keyboard.key(time, Esc, pressed)
    │◄─────────────────┤
    │                  │
    │ wl_keyboard.key(Esc, pressed) → 编辑器
```

这就是 §3 虚拟键盘的典型用法：让 IME 在 "不吃这个键" 时把它回注给焦点客户端。

### 4.7 焦点切换到另一个文本框

```
Editor A        Compositor            IME                      Editor B
   │                 │                  │                        │
   │ (用户点 Editor B)│                 │                        │
   │                 │                  │                        │
   │ wl_keyboard.leave(A_surface)       │                        │
   │◄────────────────┤                  │                        │
   │ zwp_text_input_v3.leave(A_surface) │                        │
   │◄────────────────┤                  │                        │
   │                 │                  │                        │
   │                 │                  │ zwp_input_method_v2.   │
   │                 │                  │   deactivate            │
   │                 │                  │◄───────────────────────│ (先发 deactivate)
   │                 │                  │ done                    │
   │                 │                  │◄───────────────────────│
   │                 │                  │                        │
   │                 │                  │ (若 IME 此前 grab 了，  │
   │                 │                  │  grab 随 deactivate 自动失效)
   │                 │                  │                        │
   │                 │ wl_keyboard.enter(B_surface)              │
   │                 ├──────────────────────────────────────────►│
   │                 │ zwp_text_input_v3.enter(B_surface)        │
   │                 ├──────────────────────────────────────────►│
   │                 │                  │                        │
   │                 │ (B 需要重走 §4.2 的 enable+commit 激活流程)│
   │                 │ enable                                    │
   │                 │◄──────────────────────────────────────────┤
   │                 │ set_surrounding_text / content_type / …   │
   │                 │◄──────────────────────────────────────────┤
   │                 │ commit                                    │
   │                 │◄──────────────────────────────────────────┤
   │                 │ activate → surrounding_text → done        │
   │                 ├──────────────────►│                       │
   │                 │                  │ (IME 可再 grab 一次)    │
```

关键顺序：**deactivate 必须先于新 activate**；合成器内部维护 "当前激活的 text_input 客户端 = 0 或 1"，切换是原子的。IME 收 deactivate 后要主动丢掉之前的拼音缓冲、关候选窗。

### 4.8 summary：谁触发什么

| 触发 | 编辑器动作 | 合成器动作 | IME 动作 |
|---|---|---|---|
| 窗口聚焦 | 收 `text_input.enter` | 发 `text_input.enter` | — |
| 文本框获焦 | `enable + state + commit` | `activate + state + done` | 起候选态、可能 grab |
| 用户按字母键（grab 下） | — | `grab.key` 给 IME | 累 preedit，发 `set_preedit_string + commit` |
| 用户按空格/回车选字 | — | `grab.key` 给 IME | `set_preedit(None) + commit_string + commit` |
| 编辑器光标移动 | `set_cursor_rectangle + set_surrounding_text + commit` | `popup.text_input_rectangle`, `surrounding_text` | popup 位置、候选重算 |
| 用户按 ESC | — | `grab.key` 给 IME | `set_preedit(None)`，或 `virtual_keyboard.key(Esc)` 透传 |
| 换焦点 | 新编辑器走自己的 `enable + commit` | `deactivate → enter → (新 enable 触发的) activate` | 丢旧 buffer、重新 grab |
| 编辑器 disable | `disable + commit` | `deactivate + done` | 关闭候选、释 grab |

---

## 5. X11 XIM（Xlib / Xt 经典输入法协议）

X 没有 Wayland 式的 "协议对象"。XIM 在 X 之上用 ClientMessage + 窗口属性封出一层请求 / 事件流。架构：
- **XIM server**：IME daemon（fcitx5 `--replace -r`，ibus-x11）
- **XIM client**：Xlib / Xt 的 `XOpenIM` / `XCreateIC`，本质上是 toolkit 里的文本组件

XIM 有三个 "pre-edit" 放置策略：
- **Root-window**：preedit 显示在独立顶层窗口（最保底）
- **Over-the-spot**：preedit 跟着 caret（客户端告诉 server caret 位置，server 开无焦点窗画 preedit）
- **On-the-spot**：客户端自己画 preedit（server 只回传 commit / preedit 文本）

### 5.1 握手（client ↔ server）

```
XIM client (toolkit)            X server                 XIM server (fcitx5/ibus)
      │                             │                              │
      │ XInternAtom(@server=fcitx)  │                              │
      ├────────────────────────────►│                              │
      │ GetSelectionOwner(@server=fcitx)                           │
      ├────────────────────────────►│ ← 查询谁是 IM server        │
      │ ← owner window id           │                              │
      │                             │                              │
      │ ClientMessage(_XIM_XCONNECT,│                              │
      │   client_window)            │                              │
      ├────────────────────────────►│ SendEvent → server_window   │
      │                             ├─────────────────────────────►│
      │                             │                              │
      │ ← ClientMessage(            │                              │
      │     _XIM_XCONNECT, protos)  │                              │
      │◄────────────────────────────┤◄─────────────────────────────┤
      │                             │                              │
      │ 此后所有 XIM_* 消息都是 transport-over-property:            │
      │   ChangeProperty(_client_data, <XIM packet>)                │
      │   SendEvent(ClientMessage _XIM_MOREDATA / _XIM_PROTOCOL)    │
      │   ← ChangeProperty(_server_data, <reply>)                   │
      │   ← ClientMessage(_XIM_PROTOCOL)                            │
```

### 5.2 创建 input context + 处理按键

```
XIM client                               XIM server
    │                                         │
    │ XIM_OPEN → 得到 imid                    │
    ├────────────────────────────────────────►│
    │ ← XIM_OPEN_REPLY                        │
    │◄────────────────────────────────────────┤
    │                                         │
    │ XIM_CREATE_IC(imid, attrs:              │
    │   focus_window, client_window,          │
    │   preedit_attrs{spot_location},         │
    │   input_style=OverTheSpot)              │
    ├────────────────────────────────────────►│
    │ ← XIM_CREATE_IC_REPLY(icid)             │
    │◄────────────────────────────────────────┤
    │                                         │
    │ XIM_SET_IC_FOCUS / XIM_UNSET_IC_FOCUS   │
    ├────────────────────────────────────────►│
    │                                         │
    │ (用户按键到达 client window)            │
    │ 客户端 XFilterEvent 先交给 IM 过滤:     │
    │                                         │
    │ XIM_FORWARD_EVENT(icid, KeyPress)       │
    ├────────────────────────────────────────►│
    │                                         │ 判定:
    │                                         │ - 吃掉 → 不回 FORWARD
    │                                         │   改发 PREEDIT_DRAW / COMMIT
    │                                         │ - 透传 → 回 FORWARD_EVENT
    │                                         │
    │ ← XIM_PREEDIT_DRAW(text, chg_first,     │
    │     chg_length, caret, feedbacks)       │
    │◄────────────────────────────────────────┤
    │ ← XIM_COMMIT(string)                    │
    │◄────────────────────────────────────────┤
    │ ← XIM_FORWARD_EVENT(透传的 KeyPress)    │ ← 回路：server "不吃" 时
    │◄────────────────────────────────────────┤   服务端把事件回注给客户端
    │                                         │
    │ XFilterEvent 返回 True 表示 IM 吃了;    │
    │ 否则 toolkit 正常处理按键               │
```

关键差别 vs Wayland：
- **按键必须先送 server 再回注** — Wayland 是默认到客户端、`grab_keyboard` 才改道；X 是默认先过 IM filter
- **commit 走 XIM_COMMIT，不是 XSendEvent 合成 KeyPress** — 这是为什么纯粹靠 `XSendEvent` 注入不能替代真正的 IM
- **preedit 位置更新** 靠 `XIM_SET_IC_VALUES(spot_location, ...)`，客户端告诉 server 光标当前坐标

### 5.3 其它选择协议互动

XIM 底层还会跟剪切板 selection（`@im=fcitx`）抢 `SetSelectionOwner`，以及订阅 `XFixesSelectSelectionInput` 监听 server 上下线 — 详见 [clipboard-protocols.md §1](./clipboard-protocols.md#1-x11-icccm-selection含-xfixes--incr)。

---

## 6. X11 XIM 端到端交互时序

对应 §4 的 Wayland 版本。X 世界的 IME daemon 既是 X 客户端（拿 `@im=fcitx` selection owner）也是协议 server（处理其他客户端的 XIM 请求）— 这点和 Wayland 下 IME 作为合成器 "特权客户端" 的地位截然不同。

### 6.1 IME 上线（fcitx5/ibus-x11 daemon 启动）

```
IME daemon                     X server
    │                              │
    │ XInternAtom("@server=fcitx") │
    ├─────────────────────────────►│
    │ CreateWindow(server_window)  │ ← 一个不映射的隐藏窗口，
    ├─────────────────────────────►│   用作 XIM 消息路由目标
    │                              │
    │ SetSelectionOwner(           │
    │   @server=fcitx,             │
    │   server_window, time)       │ ← 抢占 "我是 IM server" selection
    ├─────────────────────────────►│
    │                              │
    │ ChangeProperty(              │
    │   root, XIM_SERVERS,         │
    │   APPEND, "@server=fcitx")   │ ← 广告到根窗口，让后来的
    ├─────────────────────────────►│   客户端能发现
```

### 6.2 编辑器启动 + 发现 IME

```
Editor (Xlib/GTK)                  X server                   IME daemon
      │                                 │                         │
      │ XOpenDisplay                    │                         │
      ├────────────────────────────────►│                         │
      │                                 │                         │
      │ GetProperty(root, XIM_SERVERS)  │                         │
      ├────────────────────────────────►│                         │
      │ ← ["@server=fcitx", …]          │                         │
      │◄────────────────────────────────┤                         │
      │                                 │                         │
      │ GetSelectionOwner(@server=fcitx)│                         │
      ├────────────────────────────────►│                         │
      │ ← server_window                 │                         │
      │◄────────────────────────────────┤                         │
      │                                 │                         │
      │ CreateWindow(client_window)     │                         │
      ├────────────────────────────────►│                         │
      │                                 │                         │
      │ SendEvent(ClientMessage         │                         │
      │   _XIM_XCONNECT,                │                         │
      │   data=client_window,           │                         │
      │   target=server_window)         │                         │
      ├────────────────────────────────►├────────────────────────►│
      │                                 │                         │
      │                                 │ ← ClientMessage         │
      │                                 │   _XIM_XCONNECT(        │
      │                                 │   major=2, minor=0)     │
      │◄────────────────────────────────┤◄────────────────────────┤
      │                                 │                         │
      │ (XIM_OPEN → server 返回 imid，过 property transport)      │
      │                                 │                         │
      │ XIM_OPEN request               │                         │
      ├────────────────────────────────►├────────────────────────►│
      │                                 │ ← XIM_OPEN_REPLY(imid)  │
      │◄────────────────────────────────┤◄────────────────────────┤
```

和 Wayland 的差别：
- Wayland 合成器**主动**创建 text_input_manager 全局；X 是 IME daemon **抢占 selection owner** 通知世界自己在位
- 传输用 ClientMessage 触发 + ChangeProperty 搬长数据，对大请求（IM_TRIGGER_NOTIFY 多字节）走 `_XIM_MOREDATA` 分段，类似剪切板的 INCR
- XIM_OPEN 之后双方靠 **imid**（InputMethod ID）、创建 IC 后靠 **icid** 来索引；没有 Wayland 的 wl_resource 对象模型

### 6.3 编辑框获焦点：XCreateIC + XSetICFocus

```
Editor                          X server                       IME daemon
   │                                 │                             │
   │ (点中文本框)                   │                             │
   │                                 │                             │
   │ XCreateIC(                      │                             │
   │   imid,                         │                             │
   │   XNInputStyle=OverTheSpot,     │                             │
   │   XNClientWindow=editor_toplvl, │                             │
   │   XNFocusWindow=text_widget,    │                             │
   │   XNSpotLocation={x,y},         │                             │
   │   XNPreeditAttributes=…)        │                             │
   │  → XIM_CREATE_IC request        │                             │
   ├────────────────────────────────►├────────────────────────────►│
   │                                 │ ← XIM_CREATE_IC_REPLY(icid) │
   │◄────────────────────────────────┤◄────────────────────────────┤
   │                                 │                             │
   │ XSetICFocus(ic)                 │                             │
   │  → XIM_SET_IC_FOCUS             │                             │
   ├────────────────────────────────►├────────────────────────────►│
   │                                 │                             │
   │                                 │                             │ 激活中文态,
   │                                 │                             │ 挂候选窗
```

XIM 没有 "surrounding_text" 这类协议字段 — IC 的 `XNInputStyle` 决定了是否需要客户端上报位置。OverTheSpot 要求客户端通过 `XSetICValues(XNSpotLocation=...)` 主动上报光标；OnTheSpot 要求客户端安装 preedit callback（`XNPreeditDrawCallback` 等）让 server 回调客户端画 preedit。

### 6.4 用户按 "n"：XFilterEvent 决策

```
X server                     Editor                         IME daemon
   │                            │                               │
   │ KeyPress('n')              │                               │
   ├───────────────────────────►│                               │
   │                            │                               │
   │                            │ XFilterEvent(event):          │
   │                            │   → XIM_FORWARD_EVENT         │
   │                            │      (icid, KeyPress)         │
   │                            ├──────────────────────────────►│
   │                            │                               │
   │                            │                               │ 累拼音 buf "n"
   │                            │                               │ 生成候选
   │                            │                               │
   │                            │ ← XIM_PREEDIT_DRAW(           │
   │                            │     icid,                     │
   │                            │     caret=1,                  │
   │                            │     chg_first=0, chg_len=0,   │
   │                            │     text="n",                 │
   │                            │     feedbacks=[Underline])    │
   │                            │◄──────────────────────────────┤
   │                            │                               │
   │                            │ XFilterEvent 返回 True        │
   │                            │ (toolkit 不再把 KeyPress      │
   │                            │  当作普通按键处理)            │
   │                            │                               │
   │                            │ OverTheSpot 模式: server       │
   │                            │ 自己开一个 override-redirect   │
   │                            │ 窗口在 XNSpotLocation 画 "n"   │
```

关键差异：
- 按键**先**到编辑器（`KeyPress` event），编辑器调用 `XFilterEvent` 询问 IM "你吃不吃"，本质是**同步对话**（因为 `XFilterEvent` 内部会 send `XIM_FORWARD_EVENT` 并等 `XIM_COMMIT` / `XIM_PREEDIT_DRAW` / `XIM_FORWARD_EVENT` 回来）
- 事件**没有被合成器劫持** — X 里没有合成器这个角色；server 只做 ClientMessage 投递
- `feedbacks` 是字节数组，每字节对应 preedit 里一个字符的样式（`XIMReverse | XIMUnderline | XIMHighlight | ...`）

### 6.5 按空格确认候选 "你"

```
X server                     Editor                         IME daemon
   │ KeyPress(Space)            │                               │
   ├───────────────────────────►│                               │
   │                            │ XFilterEvent:                 │
   │                            │   XIM_FORWARD_EVENT(Space)    │
   │                            ├──────────────────────────────►│
   │                            │                               │ 选中第 1 候选
   │                            │                               │
   │                            │ ← XIM_PREEDIT_DRAW(           │
   │                            │     text="", chg_first=0,     │
   │                            │     chg_len=1, caret=0)       │ ← 清空 preedit
   │                            │◄──────────────────────────────┤
   │                            │ ← XIM_COMMIT(                 │
   │                            │     icid, flags,              │
   │                            │     string="你")              │
   │                            │◄──────────────────────────────┤
   │                            │                               │
   │                            │ toolkit 把 "你" 插入文本缓冲, │
   │                            │ 光标右移                      │
   │                            │                               │
   │                            │ XSetICValues(                 │
   │                            │   XNSpotLocation={x+16, y})   │
   │                            │  → XIM_SET_IC_VALUES          │
   │                            ├──────────────────────────────►│
   │                            │                               │ 候选窗跟着移
```

与 Wayland 差别：
- `XIM_COMMIT` 把字符串**直接**送给编辑器，不经过 "合成器路由"
- 光标位置靠编辑器在每次文本变化后**主动**调 `XSetICValues` — 没有 "surrounding_text" 的上下文同步，IME 看不到编辑器里真实的前后文
- 候选窗是 IME daemon **自己建的** override-redirect X 窗口；server 不帮它定位，IME 拿客户端报的 spot location 自己算摆在哪儿

### 6.6 按 ESC：string_conversion 和 forward 回路

```
X server                     Editor                         IME daemon
   │ KeyPress(Esc)              │                               │
   ├───────────────────────────►│                               │
   │                            │ XFilterEvent: forward 给 IM   │
   │                            ├──────────────────────────────►│
   │                            │                               │ 若有 preedit:
   │                            │                               │   清 preedit, 不 commit
   │                            │ ← XIM_PREEDIT_DRAW(text="",   │
   │                            │     chg_len=preedit_len)      │
   │                            │◄──────────────────────────────┤
   │                            │ (XFilterEvent 返回 True, 按键被吃)
   │                            │                               │
   │                            │ 否则 (无 preedit, Esc 透传):  │
   │                            │ ← XIM_FORWARD_EVENT(KeyPress) │ ← IME 回注
   │                            │◄──────────────────────────────┤
   │                            │ (XFilterEvent 返回 False,     │
   │                            │  toolkit 正常处理 Esc)        │
```

"IME 决定不吃" 的路径 = 服务端回发 `XIM_FORWARD_EVENT` 把原事件塞回去。这是为什么 XIM 会有 "按键延迟" 的感觉 —— 即使 IME 决定不吃也要过一次 server-client 往返。

### 6.7 失焦 / 关闭 IC

```
Editor                         X server                       IME daemon
   │                                 │                             │
   │ XUnsetICFocus(ic)               │                             │
   │  → XIM_UNSET_IC_FOCUS           │                             │
   ├────────────────────────────────►├────────────────────────────►│
   │                                 │                             │
   │ (若编辑器关闭:)                 │                             │
   │ XDestroyIC → XIM_DESTROY_IC     │                             │
   ├────────────────────────────────►├────────────────────────────►│
   │ XCloseIM  → XIM_CLOSE           │                             │
   ├────────────────────────────────►├────────────────────────────►│
```

另一条死亡路径：IME daemon 挂了。客户端通过监听 `@server=fcitx` 的 XFixes selection notify 发现 owner 消失，触发 `XIM_ERROR` 回调或在下次 `XFilterEvent` 里降级成 "IM 不可用"。toolkit 通常会缓存 `XIM_CLOSE_REPLY` 未来到的状态，等新 daemon 上线后重新 connect。

### 6.8 Wayland vs X11 IME 交互差异一览

| 维度 | Wayland (§4) | X11 XIM (§6) |
|---|---|---|
| 按键默认流向 | 先到合成器；未 grab 则给客户端 | 先到客户端 window；客户端调 `XFilterEvent` 决定 |
| IME 吃按键机制 | IME 先 `grab_keyboard`，`grab.key` 独占 | 客户端 `XIM_FORWARD_EVENT` 把事件转给 server |
| 原子 commit | `done(serial)` 一批 | `XIM_PREEDIT_DRAW + XIM_COMMIT` 没显式 serial，靠消息顺序 |
| 光标位置 | 编辑器 `set_cursor_rectangle` + 合成器转 `text_input_rectangle` | 客户端 `XSetICValues(XNSpotLocation)` 直传 server |
| 候选窗 | `get_input_popup_surface`（IME 的 surface）+ 合成器定位 | IME daemon 自己建 override-redirect X 窗口 |
| surrounding text | `set_surrounding_text` 双向 | 没有（IME 只从 commit / preedit 自建上下文） |
| 上下线通知 | text_input_manager 作为 global，bind 失败 ↔ 不可用 | IME daemon 持 `@server=fcitx` selection；XFixes 订阅 |
| 多 IME | 每 seat 只一个 input_method_v2 客户端 | 同时多个 XIM server (`@server=fcitx`, `@server=ibus`)，客户端选 |

---

## 7. IBus / fcitx5 D-Bus 旁路通道

GTK / Qt 的 `*_IM_MODULE=ibus|fcitx` 不走 XIM 也不走 Wayland `text_input_v3`，而是应用内嵌 IM module 直接用 D-Bus 和 IME daemon 对话。

```
App (GTK/Qt with IM module)              Session bus               IME daemon (fcitx5/ibus)
      │                                       │                              │
      │ org.fcitx.Fcitx5.InputMethod1.         │                              │
      │   CreateInputContext(params)           │                              │
      ├───────────────────────────────────────►│─────────────────────────────►│
      │ ← path=/org/fcitx/InputContext/1       │                              │
      │◄───────────────────────────────────────┤◄─────────────────────────────┤
      │                                         │                              │
      │ InputContext1.FocusIn / FocusOut        │                              │
      │ InputContext1.SetCursorRect(x,y,w,h)    │                              │
      │ InputContext1.ProcessKeyEvent(          │                              │
      │   keyval, keycode, state, is_release,   │                              │
      │   time)  → bool (handled)               │                              │
      ├────────────────────────────────────────►│─────────────────────────────►│
      │ ← InputContext1.CommitString(text)      │                              │
      │ ← InputContext1.UpdateFormattedPreedit( │                              │
      │     segments, cursor)                   │                              │
      │ ← InputContext1.ForwardKey(             │                              │
      │     keyval, state, is_release)          │                              │
      │◄────────────────────────────────────────┤◄─────────────────────────────┤
```

要点：
- **完全绕开合成器 / X server**。合成器只看到一个 "这个 app 不 bind text_input_v3" 的普通 Wayland 客户端
- `ProcessKeyEvent` 的返回值决定 app 要不要继续把键传给普通按键路径；`ForwardKey` 是 "IME 决定不吃、还回给 app" 的回路
- fcitx5 与 ibus 的总线接口基本同构（`org.fcitx.Fcitx5.*` vs `org.freedesktop.IBus.*`），但不兼容
- 这是为什么 `GTK_IM_MODULE=fcitx` 下 pgtk Emacs / Firefox 的 text_input_v3 不触发 — IM 层截胡了

---

## 8. XWayland 输入法桥

XWayland 本身**不是一个 IME**。想让 XWayland 下跑的 X 客户端得到 Wayland 原生的输入法：

路径 A：**IME 跨栈** — IME daemon（fcitx5）同时跑 XIM server (§4) + D-Bus 接口 (§5) + Wayland `zwp_input_method_v2` (§2)；同一套候选词映射到不同前端。这是实际部署里的主流方案。

路径 B：**协议桥接** — Xwm 监听某个 Wayland 原生 IME 的 commit，翻译成 `XIM_COMMIT` 发给聚焦 XWayland 客户端。目前 upstream Xwayland / Mutter / wlroots / smithay **都没有实现**这条路径；也没有对应的 FDO staging 协议。

因此：

```
┌────────────────────────────────────────────────────────┐
│                      XWayland 下的 X 客户端             │
│                                                         │
│  GTK2/3 无 IM module:      → 走 XIM (§4)                │
│  GTK3 / Qt5 *_IM_MODULE=fcitx|ibus: → D-Bus (§5)        │
│  GTK4 (XWayland backend):  → XIM + (部分版本的 wayland fallback)│
│                                                         │
└────────────────────────────────────────────────────────┘
```

合成器能控制的：
- **XIM daemon 的选择 owner** 是个 X selection（`@im=fcitx` / `XIM_SERVERS`），遵循 [clipboard-protocols.md §1](./clipboard-protocols.md#1-x11-icccm-selection含-xfixes--incr) 的选择 owner 机制
- **Wayland text_input_v3 ↔ XIM 的翻译层**：合成器理论上可以自己实现 "吃下自己 seat 的 input_method_v2，翻译 commit_string → 为 XWayland 客户端发 XIM_COMMIT"，但要实现完整的 XIM server 协议栈。目前没有通用实现

实务上：用户同时装 IME 的 Wayland 前端 + XIM 前端，覆盖 Wayland 原生客户端 + XWayland 客户端。

---

## 参考协议链接

### Wayland

- **text-input-unstable-v3** — 编辑器端输入法接口
  <https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/unstable/text-input/text-input-unstable-v3.xml>
- **input-method-unstable-v2** — IME 接入合成器的接口（带 keyboard grab / popup / surrounding text）
  <https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/unstable/input-method/input-method-unstable-v2.xml>
- **virtual-keyboard-unstable-v1** — 虚拟键盘（IME 注入原始按键 / keymap）
  <https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/unstable/virtual-keyboard/virtual-keyboard-unstable-v1.xml>
- **wayland.xml** 核心协议（`wl_seat` / `wl_keyboard` focus 模型）
  <https://gitlab.freedesktop.org/wayland/wayland/-/blob/main/protocol/wayland.xml>

### X11

- **XIM protocol specification**（ClientMessage + property 传输 + forward event + preedit / status attributes）
  <https://www.x.org/releases/X11R7.7/doc/libX11/XIM/xim.html>
- **Xlib §13 "Interclient Communication Conventions"**（`XOpenIM` / `XCreateIC` / `XFilterEvent` 等 API）
  <https://www.x.org/releases/X11R7.7/doc/libX11/libX11/libX11.html>
- **XFixes extension**（用于订阅 XIM server selection 变化，和剪切板共用同一机制）
  <https://www.x.org/releases/X11R7.7/doc/fixesproto/fixesproto.txt>

### D-Bus 旁路

- **fcitx5 D-Bus 接口（InputMethod1 / InputContext1）**
  <https://codeberg.org/fcitx/fcitx5/src/branch/master/src/lib/fcitx-utils/dbus/bus.h>
  <https://fcitx-im.org/wiki/Developer>
- **IBus D-Bus 接口（org.freedesktop.IBus.*）**
  <https://github.com/ibus/ibus/blob/main/bus/dbusimpl.c>

### 参考实现

- **smithay `text_input` / `input_method`** 模块（两条协议的服务器端骨架）
  <https://docs.rs/smithay/latest/smithay/wayland/text_input/index.html>
  <https://docs.rs/smithay/latest/smithay/wayland/input_method/index.html>
- **wlroots text_input + input_method**
  <https://gitlab.freedesktop.org/wlroots/wlroots/-/tree/master/types>
- **Mutter IME 桥接**（GNOME Wayland 原生 IME 路径）
  <https://gitlab.gnome.org/GNOME/mutter/-/tree/main/src/wayland>
- **fcitx5-wayland**（IME 侧绑定 input-method-v2 + virtual-keyboard-v1 的参考）
  <https://codeberg.org/fcitx/fcitx5/src/branch/master/src/frontend/waylandim>
- **Xwayland/xwayland-keyboard-grab.c**（XWayland 键盘抓取，与 IME 无直接关系但是相关语义的桥接参考）
  <https://gitlab.freedesktop.org/xorg/xserver/-/blob/master/hw/xwayland/xwayland-keyboard-grab.c>

### 实用工具

- **fcitx5-diagnose** — 排查哪个 IM frontend 接了哪个 app
- **xprop / wayland-info** — 看 selection / globals
- **evtest** — 追 evdev 键码，对齐 virtual_keyboard 的 `key` 语义
