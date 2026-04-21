# 输入法协议时序参考

本文档只讲协议本身，不涉及任何具体合成器实现。

## 目录

1. [text_input_v3（编辑器侧，Wayland 客户端收 preedit / commit）](#1-text_input_v3编辑器侧wayland-客户端收-preedit--commit)
2. [input_method_v2（IME 侧，IME 作为 Wayland 特权客户端）](#2-input_method_v2ime-侧ime-作为-wayland-特权客户端)
3. [zwp_virtual_keyboard_v1（IME 注入按键 / keymap）](#3-zwp_virtual_keyboard_v1ime-注入按键--keymap)
4. [用 fcitx5 举例看完整交互（Wayland）](#4-用-fcitx5-举例看完整交互wayland)
5. [X11 XIM（Xlib / Xt 经典输入法协议）](#5-x11-ximxlib--xt-经典输入法协议)
6. [用 fcitx5-x11 举例看完整交互（X11）](#6-用-fcitx5-x11-举例看完整交互x11)
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

## 4. 用 fcitx5 举例看完整交互（Wayland）

场景：KDE Plasma 6，fcitx5 中文拼音，在 Kate 里输入 "你"。左边图示用户在屏幕上看到什么，右边时序图标每一步用的 Wayland 接口。

> 术语提示
> - **preedit** = 屏幕上那段带下划线的"临时待确认文字"。还没进 Kate 的文档，Ctrl+Z 撤不到。
> - **commit** = 真的把文字打进 Kate 的文档里。光标右移、能 Ctrl+Z 撤销。
> - **grab keyboard** = 输入法独占键盘，按键不再到编辑器，先给输入法。

### 4.1 启动阶段：各方绑协议对象

```
Kate (编辑器)              KWin (合成器)            fcitx5 (输入法)
     │                          │                         │
     │ wl_registry.bind          │                        │
     │   (zwp_text_input_manager_v3)                      │
     ├─────────────────────────►│                         │
     │                          │◄────────────────────────┤ wl_registry.bind
     │                          │                         │   (zwp_input_method_manager_v2)
     │                          │◄────────────────────────┤ wl_registry.bind
     │                          │                         │   (zwp_virtual_keyboard_manager_v1)
     │                          │                         │
     │ text_input_manager_v3.   │                         │
     │   get_text_input(seat)   │                         │
     ├─────────────────────────►│                         │
     │ → zwp_text_input_v3      │                         │
     │                          │ input_method_manager_v2.│
     │                          │   get_input_method(seat)│
     │                          │◄────────────────────────┤
     │                          │ → zwp_input_method_v2   │
     │                          │                         │
     │                          │ virtual_keyboard.       │
     │                          │   keymap(fd, size)      │
     │                          │◄────────────────────────┤
```

### 4.2 点中 Kate 文本框：激活输入会话

屏幕变化：光标闪进文本框，状态栏仍是 "EN"，候选窗还没出现。

```
Kate                       KWin                       fcitx5
 │                          │                          │
 │ (用户鼠标点 Kate 文本框) │                          │
 │                          │                          │
 │ wl_keyboard.enter        │                          │
 │   (surface, keys)        │                          │
 │◄─────────────────────────┤                          │
 │ zwp_text_input_v3.enter  │                          │
 │   (surface)              │                          │
 │◄─────────────────────────┤                          │
 │                          │                          │
 │ text_input_v3.enable     │                          │
 ├─────────────────────────►│                          │
 │ text_input_v3.           │                          │
 │   set_surrounding_text   │                          │
 │   (text, cursor, anchor) │                          │
 ├─────────────────────────►│                          │
 │ text_input_v3.           │                          │
 │   set_content_type       │                          │
 │   (hint, purpose)        │                          │
 ├─────────────────────────►│                          │
 │ text_input_v3.           │                          │
 │   set_cursor_rectangle   │                          │
 │   (x, y, w, h)           │                          │
 ├─────────────────────────►│                          │
 │ text_input_v3.commit     │ ← 原子落盘               │
 ├─────────────────────────►│                          │
 │                          │                          │
 │                          │ input_method_v2.activate │
 │                          ├─────────────────────────►│
 │                          │ input_method_v2.         │
 │                          │   surrounding_text(…)    │
 │                          ├─────────────────────────►│
 │                          │ input_method_v2.         │
 │                          │   content_type(…)        │
 │                          ├─────────────────────────►│
 │                          │ input_method_v2.done     │
 │                          ├─────────────────────────►│
 │                          │                          │
 │                          │ input_method_v2.         │
 │                          │   grab_keyboard          │
 │                          │◄─────────────────────────┤
 │                          │ → zwp_input_method_keyboard_grab_v2
 │                          │                          │
 │                          │ grab.keymap(fd, size)    │
 │                          ├─────────────────────────►│
```

此后键盘按键不再走 `wl_keyboard.key` 给 Kate，改走 `grab.key` 给 fcitx5。

### 4.3 按 n：屏幕出现下划线 `n` + 候选窗

```
屏幕变化:                              KWin                        fcitx5
┌────────────────┐                      │                           │
│ n│             │                      │ grab.modifiers(…)         │
│ ‾              │                      ├──────────────────────────►│
│┌──────────────┐│                      │ grab.key(serial, time,    │
││1.你 2.能 3.呢 ││                     │   key='n', state=pressed) │
││4.哪 5.那  >  ││                      ├──────────────────────────►│
│└──────────────┘│                      │                           │
└────────────────┘                      │                           │ (拼音 buf='n',
                                        │                           │  候选生成)
                                        │                           │
                                        │ input_method_v2.          │
                                        │   set_preedit_string(     │
                                        │     text='n',             │
                                        │     cursor_begin=0,       │
                                        │     cursor_end=1)         │
                                        │◄──────────────────────────┤
                                        │ input_method_v2.commit    │
                                        │   (serial=S)              │
                                        │◄──────────────────────────┤
      Kate                              │                           │
       │ zwp_text_input_v3.             │                           │
       │   preedit_string(              │                           │
       │     text='n', begin=0, end=1)  │                           │
       │◄───────────────────────────────┤                           │
       │ zwp_text_input_v3.done(S)      │                           │
       │◄───────────────────────────────┤                           │
       │                                │                           │
       │ (Kate 在光标位置画下划线 'n')  │                           │
       │                                │                           │
      候选窗 surface                    │                           │
       │                                │ (fcitx5 把候选窗          │
       │ zwp_input_popup_surface_v2.    │  注册为特殊 popup)        │
       │   text_input_rectangle(        │                           │
       │     x, y, w, h)                │                           │
       │◄───────────────────────────────┤                           │
       │ wl_surface.commit (buffer)     │                           │
       ├────────────────────────────────┼──────────────────────────►│
       │                                │                           │
       │ (KWin 把候选窗放在 Kate 光标下方)
```

### 4.4 按 i：下划线扩成 `ni`

协议层只是重复 §4.3 的 `grab.key` → `set_preedit_string('ni', 0, 2)` → `commit(S+1)` → `preedit_string → done(S+1)`。Kate 整段替换下划线。

注意：**每次 `set_preedit_string` 是全量替换**（不是追加）；fcitx5 要自己记当前临时文字。

### 4.5 按空格：真把 "你" 打进 Kate

```
屏幕变化:                              KWin                        fcitx5
┌────────────────┐                      │                           │
│ 你│            │                      │ grab.key(Space, pressed)  │
│                │                      ├──────────────────────────►│
│                │                      │                           │ (选第 1 候选 "你")
│                │                      │                           │
└────────────────┘                      │ input_method_v2.          │
                                        │   set_preedit_string(     │
                                        │     text=None, 0, 0)      │ ← 清 preedit
                                        │◄──────────────────────────┤
                                        │ input_method_v2.          │
                                        │   commit_string(          │
                                        │     text="你")            │
                                        │◄──────────────────────────┤
                                        │ input_method_v2.commit    │
                                        │   (serial=S+2)            │
                                        │◄──────────────────────────┤
       Kate                             │                           │
        │ text_input_v3.                │                           │
        │   preedit_string(None, 0, 0)  │                           │
        │◄──────────────────────────────┤                           │
        │ text_input_v3.                │                           │
        │   commit_string("你")         │                           │
        │◄──────────────────────────────┤                           │
        │ text_input_v3.done(S+2)       │                           │
        │◄──────────────────────────────┤                           │
        │                               │                           │
        │ (Kate: 擦下划线 → 真插入 "你" │                           │
        │   → 光标右移一格)             │                           │
        │                               │                           │
        │ text_input_v3.                │                           │
        │   set_cursor_rectangle(       │                           │
        │     新坐标)                   │                           │
        ├──────────────────────────────►│                           │
        │ text_input_v3.                │                           │
        │   set_surrounding_text(       │                           │
        │     更新后的文字, 光标位置)   │                           │
        ├──────────────────────────────►│                           │
        │ text_input_v3.commit          │                           │
        ├──────────────────────────────►│                           │
        │                               │ input_method_v2.          │
        │                               │   surrounding_text(…)     │
        │                               ├──────────────────────────►│
        │                               │ input_method_v2.done      │
        │                               ├──────────────────────────►│
```

要点：`preedit_string(None) + commit_string("你") + done(S+2)` 必须在**同一批**里发（同一个 done 之间），Kate 才会原子地 "擦下划线 + 插真字"。否则屏幕会瞬间出现 "ni你"。

### 4.6 按 ESC：取消组字（不 commit）

```
KWin                       fcitx5                    Kate
 │ grab.key(Esc, pressed)   │                          │
 ├─────────────────────────►│                          │
 │                          │ (清拼音 buf，关候选窗)   │
 │                          │                          │
 │ input_method_v2.         │                          │
 │   set_preedit_string(    │                          │
 │     None, 0, 0)          │                          │
 │◄─────────────────────────┤                          │
 │ input_method_v2.commit   │                          │
 │◄─────────────────────────┤                          │
 │                          │                          │
 │ text_input_v3.           │                          │
 │   preedit_string(        │                          │
 │     None, 0, 0)          │                          │
 ├──────────────────────────────────────────────────►│
 │ text_input_v3.done       │                          │
 ├──────────────────────────────────────────────────►│
 │                          │                          │
 │                          │ (Vim 等想要 Esc 透传时:)│
 │ virtual_keyboard_v1.     │                          │
 │   key(time, Esc, pressed)│                          │
 │◄─────────────────────────┤                          │
 │                          │                          │
 │ wl_keyboard.key(Esc)     │                          │
 ├──────────────────────────────────────────────────►│ (Kate 收到原始 Esc)
```

`virtual_keyboard.key` 这条路是可选的 — Vim 里要用 Esc 退插入模式就走这条；普通编辑器 fcitx5 默认不透传。

### 4.7 焦点切换

```
Kate A         KWin                     fcitx5               Kate B
  │             │                         │                    │
  │             │ (用户点 Kate B)         │                    │
  │ wl_keyboard.leave(A)                  │                    │
  │◄────────────┤                         │                    │
  │ text_input_v3.leave(A)                │                    │
  │◄────────────┤                         │                    │
  │             │ input_method_v2.        │                    │
  │             │   deactivate            │                    │
  │             ├────────────────────────►│                    │
  │             │ input_method_v2.done    │                    │
  │             ├────────────────────────►│                    │
  │             │                         │ (grab 随 deactivate 自动失效)
  │             │                         │                    │
  │             │ wl_keyboard.enter(B)    │                    │
  │             ├───────────────────────────────────────────►│
  │             │ text_input_v3.enter(B)  │                    │
  │             ├───────────────────────────────────────────►│
  │             │                         │                    │
  │             │                         │       text_input_v3.enable
  │             │                         │       text_input_v3.commit
  │             │◄───────────────────────────────────────────┤
  │             │                         │                    │
  │             │ (Kate B 若也要 IME,     │                    │
  │             │  重走 §4.2 激活流程)    │                    │
```

### 4.8 fcitx5 vs ibus

协议调用**完全相同**，区别只在：
- **fcitx5-wayland**：Arch/KDE 默认，候选窗横排
- **ibus-wayland**：Fedora/GNOME 默认，候选窗竖排
- 一个 seat 只能绑**一个** `zwp_input_method_v2`，两者不能同时生效

### 4.9 一句话回顾

Kate 用 `zwp_text_input_v3` 告诉 KWin "我要输入文字、光标在哪、前面是什么"；fcitx5 用 `zwp_input_method_v2` 从 KWin 拿到这些信息 + 键盘独占，算完拼音后又通过 KWin 把 preedit / commit_string 发回 Kate。KWin 只做协议对象中转，不理解文字。

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

## 6. 用 fcitx5-x11 举例看完整交互（X11）

对应 §4 的 Wayland 场景。用户画面**完全一样**（下划线 `ni` + 候选窗 + 空格出 "你"），底下协议完全不同。每条消息都标出 X 原语名。

### 6.1 fcitx5 daemon 上线：抢 `@server=fcitx` selection

```
fcitx5 daemon                      X server
     │                                 │
     │ XInternAtom("@server=fcitx")    │
     ├────────────────────────────────►│
     │ XCreateWindow(server_window)    │
     ├────────────────────────────────►│
     │ XSetSelectionOwner(             │
     │   selection=@server=fcitx,      │
     │   owner=server_window, time)    │
     ├────────────────────────────────►│
     │ XChangeProperty(                │
     │   root, XIM_SERVERS,            │
     │   ATOM, PropModeAppend,         │
     │   data=["@server=fcitx"])       │
     ├────────────────────────────────►│
```

### 6.2 Kate 发现 fcitx5 + XIM 握手

```
Kate                      X server                      fcitx5 daemon
  │                           │                              │
  │ XGetWindowProperty(       │                              │
  │   root, XIM_SERVERS)      │                              │
  ├──────────────────────────►│                              │
  │ ← ["@server=fcitx", …]    │                              │
  │◄──────────────────────────┤                              │
  │                           │                              │
  │ XGetSelectionOwner(       │                              │
  │   @server=fcitx)          │                              │
  ├──────────────────────────►│                              │
  │ ← server_window           │                              │
  │◄──────────────────────────┤                              │
  │                           │                              │
  │ XCreateWindow(client_window)                             │
  ├──────────────────────────►│                              │
  │ XSendEvent(               │                              │
  │   server_window,          │                              │
  │   ClientMessage {         │                              │
  │     type=_XIM_XCONNECT,   │                              │
  │     data[0]=client_window │                              │
  │   })                      │                              │
  ├──────────────────────────►├─────────────────────────────►│
  │                           │ ← ClientMessage {            │
  │                           │   type=_XIM_XCONNECT,        │
  │                           │   data=[major=2, minor=0]    │
  │                           │ }                            │
  │◄──────────────────────────┤◄─────────────────────────────┤
  │                           │                              │
  │ (后续 XIM 消息用 "property transport": )                 │
  │ XChangeProperty(          │                              │
  │   client_window, _client_data, <XIM_OPEN packet>)        │
  ├──────────────────────────►│                              │
  │ XSendEvent(server_window, │                              │
  │   ClientMessage _XIM_PROTOCOL)                           │
  ├──────────────────────────►├─────────────────────────────►│
  │                           │ ← XChangeProperty(           │
  │                           │     _server_data,            │
  │                           │     <XIM_OPEN_REPLY(imid)>)  │
  │                           │ ← ClientMessage _XIM_PROTOCOL│
  │◄──────────────────────────┤◄─────────────────────────────┤
```

### 6.3 点中文本框：`XCreateIC + XSetICFocus`

```
Kate                                                    fcitx5 daemon
  │                                                           │
  │ (用户点文本框)                                            │
  │                                                           │
  │ XCreateIC(                                                │
  │   imid,                                                   │
  │   XNInputStyle=XIMPreeditPosition | XIMStatusNothing,     │
  │   XNClientWindow=editor_toplevel,                         │
  │   XNFocusWindow=text_widget,                              │
  │   XNSpotLocation={x=200,y=100})                           │
  │ → 封装为 XIM_CREATE_IC 消息                               │
  ├──────────────────────────────────────────────────────────►│
  │ ← XIM_CREATE_IC_REPLY(icid=7)                             │
  │◄──────────────────────────────────────────────────────────┤
  │                                                           │
  │ XSetICFocus(ic)                                           │
  │ → XIM_SET_IC_FOCUS(icid=7)                                │
  ├──────────────────────────────────────────────────────────►│
  │                                                           │ (激活中文态)
```

### 6.4 按 n：`XFilterEvent` 同步往返

```
屏幕变化:                 X server         Kate (Xlib)         fcitx5
┌────────────────┐            │                │                  │
│ n│             │            │ KeyPress('n')  │                  │
│ ‾              │            ├───────────────►│                  │
│┌──────────────┐│            │                │ XFilterEvent(event):
││1.你 2.能 3.呢 ││           │                │ → XIM_FORWARD_EVENT
││4.哪 5.那  >  ││            │                │   (icid, serialized KeyPress)
│└──────────────┘│            │                ├─────────────────►│
└────────────────┘            │                │                  │ (拼音 n, 候选)
                              │                │                  │
                              │                │ ← XIM_PREEDIT_DRAW(
                              │                │     icid,
                              │                │     chg_first=0,
                              │                │     chg_length=0,
                              │                │     text='n',
                              │                │     caret=1,
                              │                │     feedbacks=[Underline])
                              │                │◄─────────────────┤
                              │                │                  │
                              │                │ (fcitx5 没回 XIM_FORWARD_EVENT
                              │                │  → Kate 认为 IME 吃了这个键)
                              │                │                  │
                              │                │ XFilterEvent 返回 True
                              │                │ (Kate 不当普通按键处理)
                              │                │                  │
                              │                │ (PreeditPosition 模式:
                              │                │  fcitx5 自己调 XCreateWindow
                              │                │  画候选窗)
                              │ ← XCreateWindow(override-redirect, 候选窗)
                              │◄─────────────────────────────────┤
                              │ ← XMapWindow(候选窗)             │
                              │◄─────────────────────────────────┤
```

vs Wayland 的三个本质差别：

1. `KeyPress` 先到 Kate（X server 路由），Kate 主动调 `XFilterEvent` 询问 IME
2. `XFilterEvent` 是**同步阻塞**的 — 它内部 `XSendEvent` + 等 `XIM_PREEDIT_DRAW` / `XIM_COMMIT` / `XIM_FORWARD_EVENT` 回来才返回
3. "IME 吃了这个键" 的表达：服务端**不回** `XIM_FORWARD_EVENT`；服务端要"不吃"就得回一个 `XIM_FORWARD_EVENT` 把原事件塞回来

### 6.5 按空格：`XIM_COMMIT` 真插入

```
X server          Kate                               fcitx5
    │ KeyPress(Space)                                  │
    ├────────────►│                                    │
    │             │ XFilterEvent → XIM_FORWARD_EVENT   │
    │             ├───────────────────────────────────►│
    │             │                                    │ (选第 1 候选 "你")
    │             │                                    │
    │             │ ← XIM_PREEDIT_DRAW(                │
    │             │     chg_length=2, text="")         │ ← 清 preedit
    │             │◄───────────────────────────────────┤
    │             │ ← XIM_COMMIT(                      │
    │             │     icid, flags=0,                 │
    │             │     string="你")                   │
    │             │◄───────────────────────────────────┤
    │             │                                    │
    │             │ XFilterEvent 返回 True             │
    │             │ (Kate: 清下划线; 插 "你"; 光标右移)│
    │             │                                    │
    │             │ XSetICValues(                      │
    │             │   XNSpotLocation={x=216, y=100})   │
    │             │ → XIM_SET_IC_VALUES(icid, attrs)   │
    │             ├───────────────────────────────────►│
    │                                                  │ (候选窗 XMoveWindow 到新位置)
```

X 没有 "surrounding_text" — fcitx5 只从自己发的 commit 字符串累积上下文，看不见 Kate 真实的前后文。

### 6.6 按 ESC：清 preedit，不 commit

```
X server          Kate                               fcitx5
    │ KeyPress(Esc)                                    │
    ├────────────►│                                    │
    │             │ XFilterEvent → XIM_FORWARD_EVENT   │
    │             ├───────────────────────────────────►│
    │             │                                    │
    │             │                                    │ 有 preedit:
    │             │                                    │   清 preedit, 不 COMMIT
    │             │ ← XIM_PREEDIT_DRAW(                │
    │             │     chg_length=preedit_len,        │
    │             │     text="")                       │
    │             │◄───────────────────────────────────┤
    │             │                                    │ （不回 FORWARD_EVENT
    │             │                                    │  → Kate 视为 IME 吃了）
    │             │                                    │
    │             │                                    │ 无 preedit: 回注原事件
    │             │ ← XIM_FORWARD_EVENT(               │
    │             │     icid, serialized KeyPress Esc) │
    │             │◄───────────────────────────────────┤
    │             │                                    │
    │             │ XFilterEvent 返回 False            │
    │             │ (Kate 当普通 Esc 处理 — Vim 退出   │
    │             │  insert mode)                      │
```

这就是 §4.6 里 Wayland `virtual_keyboard.key` 的 X11 对应：X 下是 `XIM_FORWARD_EVENT` 回路，Wayland 下是独立的 virtual_keyboard 协议对象。

### 6.7 失焦 / fcitx5 daemon 挂掉

```
Kate                                                    fcitx5 daemon
  │ XUnsetICFocus(ic)                                         │
  │ → XIM_UNSET_IC_FOCUS(icid)                                │
  ├──────────────────────────────────────────────────────────►│
  │                                                           │
  │ (Kate 关闭:)                                              │
  │ XDestroyIC(ic) → XIM_DESTROY_IC                           │
  ├──────────────────────────────────────────────────────────►│
  │ XCloseIM(im)   → XIM_CLOSE                                │
  ├──────────────────────────────────────────────────────────►│
```

另一条死亡路径：daemon 进程挂了。Kate 启动时对 `@server=fcitx` 订阅了 `XFixesSelectSelectionInput`（XFixes 扩展），selection owner 消失时会收 `XFixesSelectionNotify`，Kate 把 IME 标成不可用；之后 `XFilterEvent` 直接返回 `False`，按键走普通路径。新 daemon 上线重抢 selection，Kate 收通知重连。

### 6.8 为什么 fcitx5 同时跑 Wayland + X11 + D-Bus 三套前端

因为一台机器上混着不同类型应用：
- **Kate / Firefox Wayland 模式** → 走 §4 `zwp_text_input_v3`
- **WPS（X11，跑在 XWayland 下）** → 走 §6 XIM
- **带 `GTK_IM_MODULE=fcitx` 的 GTK app** → 走 §7 D-Bus

fcitx5 daemon 同时开这三个前端，共享同一词库；用户切一次输入法，三条路径同时生效。

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
