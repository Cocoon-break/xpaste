# 使用说明（docs/使用说明.md）的配图：用 website/demo 的真实前端加演示数据截图，深浅两套，2 倍像素。
# 界面改了之后重新生成：
#   make site-demo
#   python3 scripts/guide-shots.py website/demo /tmp/guide          # 需要 pip install playwright && playwright install chromium
#   for f in /tmp/guide/*.png; do magick "$f" -quality 80 -define webp:method=6 "docs/screenshots/guide/$(basename "$f" .png).webp"; done
# 只重截某几张：在命令末尾写上名字，如 ... /tmp/guide menu shot
import sys
from pathlib import Path
from playwright.sync_api import sync_playwright

demo, out = Path(sys.argv[1]).resolve(), Path(sys.argv[2])
only = sys.argv[3:]  # 只截这几张（调试用）
out.mkdir(parents=True, exist_ok=True)
W, H = 960, 640


def panel(pg, theme, extra=''):
    pg.goto(f'file://{demo}/index.html?theme={theme}{extra}')
    pg.wait_for_selector('#rows .item')
    pg.wait_for_timeout(500)
    pg.click('#q')


def type_q(pg, text):
    pg.keyboard.type(text, delay=30)
    pg.wait_for_timeout(700)


def row(pg, text):
    return pg.locator('#rows .item', has_text=text).first


SHOTS = {}


def shot(name, size=(W, H)):
    def deco(f):
        SHOTS[name] = (f, size)
        return f
    return deco


@shot('panel')
def _(pg, t):
    panel(pg, t)


@shot('search-pinyin')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    type_q(pg, 'hy')


@shot('search-ocr')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    type_q(pg, '会议号')


@shot('search-file')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    type_q(pg, '灰度方案')


@shot('search-source')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    type_q(pg, '微信')


@shot('search-source-cap')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    type_q(pg, 'wx')
    pg.keyboard.press('Tab')
    pg.wait_for_timeout(300)
    type_q(pg, '会议')


@shot('spectrum')
def _(pg, t):
    panel(pg, t)
    pg.click('#spPresets button[data-span="3600000"]')
    pg.wait_for_timeout(900)


@shot('menu')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    r = row(pg, '周四下午')
    r.click()
    r.click(button='right')
    pg.wait_for_timeout(500)


@shot('multi')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    row(pg, '上海市徐汇区').click()
    row(pg, '周四下午').click(modifiers=['Meta'])
    row(pg, '好的，我晚点').click(modifiers=['Meta'])
    pg.wait_for_timeout(500)


@shot('snippets')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    pg.click('#tabsBase button[data-f="snip"]')
    pg.wait_for_timeout(700)


@shot('translate')
def _(pg, t):
    panel(pg, t, '&spectrum=0')
    row(pg, '周四下午').click()
    pg.wait_for_timeout(300)
    pg.keyboard.press('Meta+t')
    pg.wait_for_timeout(2500)


@shot('compact', (420, 540))
def _(pg, t):
    panel(pg, t, '&style=compact')
    type_q(pg, '会议')


@shot('settings')
def _(pg, t):
    panel(pg, t)
    pg.keyboard.press('Meta+,')
    pg.wait_for_timeout(900)


@shot('shot', (960, 600))
def _(pg, t):
    pg.goto(f'file://{demo}/shot.html?theme={t}&scene=shot')
    pg.wait_for_timeout(1200)
    # 让演示脚本自己框选、画矩形、画箭头；按回车（约 6.6 秒）之前截
    pg.evaluate("() => postMessage({ xp: true, scene: 'shot', type: 'play' }, '*')")
    pg.wait_for_timeout(6000)


@shot('card', (560, 260))
def _(pg, t):
    pg.goto(f'file://{demo}/card.html?theme={t}&scene=card')
    pg.wait_for_timeout(600)
    pg.evaluate("""() => postMessage({ xp: true, scene: 'card', type: 'card',
      source: 'The dry run shows 34 new packages with no peer conflicts, and nothing already installed gets changed.',
      text: '试运行显示 34 个新软件包，没有对等依赖冲突，已安装的内容都不会被改动。' }, '*')""")
    pg.wait_for_timeout(3000)


with sync_playwright() as p:
    b = p.chromium.launch()
    errs = []
    for name, (f, (w, h)) in SHOTS.items():
        if only and name not in only:
            continue
        for t in ['dark', 'light']:
            ctx = b.new_context(viewport={'width': w, 'height': h}, device_scale_factor=2, color_scheme=t)
            pg = ctx.new_page()
            pg.on('pageerror', lambda e, n=name: errs.append(f'{n}: {e}'))
            f(pg, t)
            pg.screenshot(path=str(out / f'{name}-{t}.png'), omit_background=True)
            ctx.close()
        print('ok', name)
    print('errors', errs)
    b.close()
