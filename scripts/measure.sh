#!/usr/bin/env bash
# macOS 性能与可靠性测量（make measure）。兼容 macOS 自带 bash 3.2。
# 用法：bash scripts/measure.sh <startup|latency|summon|search|crash|idle|all>
# 被测程序 $APP 每次都用 mktemp -d 下种子目录的新副本作数据目录（XPASTE_DATA_DIR），
# 不碰真实数据目录，只比较真实库的修改时间。结果写 target/measure/<时间>.txt。
set -euo pipefail
cd "$(dirname "$0")/.."

APP=${APP:-target/release/bundle/macos/xPaste.app/Contents/MacOS/xPaste}
REAL_DB="$HOME/Library/Application Support/xPaste/xpaste.db"
USAGE="用法：bash scripts/measure.sh <startup|latency|summon|search|crash|idle|all>"
case ${1:-} in
  startup | latency | summon | search | crash | idle) STEPS=$1 ;;
  all) STEPS="startup latency summon search crash idle" ;;
  *) echo "$USAGE" >&2 && exit 2 ;;
esac

# —— 公共前置 ——
[[ $(uname) == Darwin ]] || { echo "只支持 macOS" >&2; exit 1; }
[[ -x $APP ]] || { echo "找不到 ${APP}，请先 make release-app" >&2; exit 1; }
if pgrep -x xPaste >/dev/null; then
  echo "检测到正在运行的 xPaste，请先从菜单栏退出" >&2
  exit 1
fi
# 需要按键的子命令只在离开电脑时运行（idle 也要唤起一次）。合成按键会重置空闲时间，所以只在开头检查
case " $STEPS " in *" summon "* | *" search "* | *" crash "* | *" idle "*)
  idle_s=$(ioreg -c IOHIDSystem | awk '/HIDIdleTime/ { v = int($NF / 1000000000) } END { print v + 0 }')
  if ((idle_s < 20)); then
    echo "请在离开电脑时运行（键盘鼠标已空闲 ${idle_s} 秒，需要 ≥ 20 秒）" >&2
    exit 1
  fi
  ;;
esac

mkdir -p target/measure
OUT=target/measure/$(date +%Y%m%d-%H%M%S).txt
exec > >(tee "$OUT") 2>&1

cargo build -q -p xpaste-core --release --example seed
SEED=target/release/examples/seed

ROOT=$(mktemp -d)
CLIP=$(pbpaste; printf x)
CLIP=${CLIP%x}
REAL_MTIME=$(stat -f %m "$REAL_DB" 2>/dev/null || true)
AGENTS=$(ls "$HOME/Library/LaunchAgents" 2>/dev/null || true)
PID="" KIDS_DONE="" BEFORE="" T0=0 DIR="" RUNS=0 DOC="" WATCH=""
: >"$ROOT/pids"
: >"$ROOT/net.log"

finish() {
  local rc=$?
  set +e
  stop_app || rc=1
  [[ -n $WATCH ]] && kill "$WATCH" 2>/dev/null && wait "$WATCH" 2>/dev/null
  [[ -n $DOC ]] && osascript -e "tell application \"TextEdit\" to close (every document whose name is \"$DOC\") saving no" >/dev/null 2>&1
  printf '%s' "$CLIP" | pbcopy
  echo "结果 网络连接 次数=$(wc -l <"$ROOT/net.log" | tr -d ' ') 目标=0"
  sed 's/^/  /' "$ROOT/net.log"
  if [[ -z $REAL_MTIME ]]; then
    echo "真实数据库不存在，未比较"
  elif [[ $(stat -f %m "$REAL_DB" 2>/dev/null) == "$REAL_MTIME" ]]; then
    echo "真实数据库未改动"
  else
    echo "失败：真实数据库修改时间变了" && rc=1
  fi
  if [[ $(ls "$HOME/Library/LaunchAgents" 2>/dev/null) == "$AGENTS" ]]; then
    echo "LaunchAgents 未改动"
  else
    echo "失败：~/Library/LaunchAgents 有变化" && rc=1
  fi
  rm -rf "$ROOT"
  echo "结果文件：$OUT"
  exit "$rc"
}
trap finish EXIT

ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }
q() { sqlite3 -cmd '.timeout 2000' "$DIR/xpaste.db" "$1"; }
# 记录数会被 max_records 截住（5000 条时新增一条再删最旧一条），用自增 id 判断有没有新记录入库
max_id() { q 'select coalesce(max(id), 0) from records'; }
log_count() { cat "$DIR"/logs/*.log 2>/dev/null | grep -c -- "$1" || true; }
# log_values 前缀：取 “前缀 N ms” 里的 N
log_values() { cat "$DIR"/logs/*.log 2>/dev/null | grep -o -- "$1 [0-9]* ms" | awk '{ print $2 }' || true; }

# wait_log 文本 [超时秒] [至少几行]
wait_log() {
  local deadline=$(($(ms) + ${2:-30} * 1000))
  while (($(log_count "$1") < ${3:-1})); do
    if (($(ms) > deadline)); then
      echo "超时：日志未出现「$1」"
      return 1
    fi
    sleep 0.1
  done
}

# seed 条数：DIR = 该条数种子目录的一份新副本
seed() {
  [[ -d $ROOT/seed-$1 ]] || "$SEED" "$ROOT/seed-$1" "$1"
  RUNS=$((RUNS + 1))
  DIR=$ROOT/run-$RUNS
  cp -R "$ROOT/seed-$1" "$DIR"
}

launch() {
  BEFORE=$(pgrep -f com.apple.WebKit | sort || true)
  T0=$(ms)
  XPASTE_DATA_DIR="$DIR" XPASTE_TIMING=1 "$APP" >/dev/null 2>&1 &
  PID=$!
  KIDS_DONE=""
  echo "$PID" >"$ROOT/pids"
}

# 启动后 5 秒内新出现的 WebKit 进程算作被测程序的 WebView 子进程
find_kids() {
  local left=$((T0 + 5000 - $(ms)))
  if ((left > 0)); then sleep "$(awk -v l="$left" 'BEGIN { print l / 1000 }')"; fi
  { echo "$PID"; comm -13 <(echo "$BEFORE") <(pgrep -f com.apple.WebKit | sort || true); } | sed '/^$/d' >"$ROOT/pids"
  KIDS_DONE=1
  echo "  WebKit 子进程：$(tail -n +2 "$ROOT/pids" | paste -sd, -)"
}

# stop_app [信号]：结束自己启动的 PID 并确认没有残留
stop_app() {
  [[ -n $PID ]] || return 0
  [[ -n $KIDS_DONE ]] || find_kids
  local kids
  kids=$(tail -n +2 "$ROOT/pids")
  kill "-${1:-TERM}" "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null || true
  PID=""
  : >"$ROOT/pids"
  local i
  for i in $(seq 50); do
    pgrep -x xPaste >/dev/null || break
    sleep 0.1
  done
  for i in $(seq 50); do
    [[ -n $kids ]] && ps -p "$(echo "$kids" | paste -sd, -)" >/dev/null 2>&1 || break
    sleep 0.1
  done
  grep -hE 'XPASTE_TIMING|bootstrap 返回|设置已保存|\[(WARN|ERROR)\]' "$DIR"/logs/*.log 2>/dev/null | sed 's/^/  日志 /' || true
  if pgrep -x xPaste >/dev/null; then
    echo "失败：xPaste 退出后仍有残留进程"
    return 1
  fi
}

watch_net() {
  local pids
  while sleep 2; do
    pids=$(paste -sd, "$ROOT/pids" 2>/dev/null) || continue
    [[ -n $pids ]] || continue
    lsof -nP -i -a -p "$pids" 2>/dev/null | tail -n +2 >>"$ROOT/net.log" || true
  done
}
watch_net &
WATCH=$!

key() { osascript -e "tell application \"System Events\" to $1"; }
HOTKEY='key code 9 using {command down, shift down}'

open_doc() {
  [[ -z $DOC ]] || return 0
  DOC=xpaste-measure-$$.txt
  : >"$ROOT/$DOC"
  open -a TextEdit "$ROOT/$DOC"
  sleep 1
  osascript -e "tell application \"TextEdit\" to activate"
  sleep 1.5
}

# 每批按键前：前台必须是文本编辑且前台文稿是脚本打开的那份，否则中止
front_ok() {
  local app doc
  app=$(osascript -e 'tell application "System Events" to get bundle identifier of first application process whose frontmost is true' 2>/dev/null || true)
  doc=$(osascript -e 'tell application "TextEdit" to get name of front document' 2>/dev/null || true)
  if [[ $app != com.apple.TextEdit || $doc != "$DOC" ]]; then
    echo "中止：前台不是文本编辑的测量文稿（$app / ${doc}）"
    exit 1
  fi
}

# summary 项目 单位 目标(- 不判定) 判定(le|lt|p95) 最少样本数 样本...
summary() {
  local name=$1 unit=$2 target=$3 mode=$4 min=$5
  shift 5
  printf '%s\n' "$@" | awk -v name="$name" -v u="$unit" -v t="$target" -v mode="$mode" -v min="$min" '
    NF { a[++n] = $1 + 0 }
    END {
      if (n == 0) { printf "结果 %s n=0 目标=%s%s 达标=否\n", name, t, u; exit }
      for (i = 2; i <= n; i++) { v = a[i]; for (j = i - 1; j >= 1 && a[j] > v; j--) a[j + 1] = a[j]; a[j + 1] = v }
      med = (n % 2) ? a[(n + 1) / 2] : (a[n / 2] + a[n / 2 + 1]) / 2
      max = a[n]
      p = a[int((95 * n + 99) / 100)]
      if (t == "-") ok = "不判定"
      else if (n < min) ok = "否"
      else if (mode == "lt") ok = (max < t) ? "是" : "否"
      else if (max <= t) ok = "是"
      else if (mode == "p95" && med <= t && p <= t) ok = "部分"
      else ok = "否"
      extra = (mode == "p95") ? sprintf(" p95=%g%s", p, u) : ""
      printf "结果 %s n=%d 中位数=%g%s%s 最大=%g%s 目标=%s%s 达标=%s\n", name, n, med, u, extra, max, u, t, (t == "-") ? "" : u, ok
      if (n < min) printf "  样本数 %d 少于 %d\n", n, min
    }'
  echo "  样本（${unit}）：$*"
}

# —— 子命令 ——
ROUND=0
run_startup() {
  local n s t base samples
  for n in 0 5000; do
    samples=""
    for _ in 1 2 3 4 5; do
      ROUND=$((ROUND + 1))
      seed "$n"
      base=$(max_id)
      launch
      s=0 t=""
      while (($(ms) - T0 < 30000)); do
        s=$((s + 1))
        printf 'xpaste 测量 启动 %s %s' "$ROUND" "$s" | pbcopy
        sleep 0.1
        if (($(max_id) > base)); then
          t=$(($(ms) - T0))
          break
        fi
      done
      if [[ -n $t ]]; then samples="$samples $t"; else echo "第 $ROUND 轮 30 秒内未入库"; fi
      stop_app
    done
    # shellcheck disable=SC2086
    summary "启动(${n}条)" ms 2000 le 0 $samples
  done
}

run_latency() {
  local i base t0 t samples=""
  seed 5000
  launch
  wait_log 'bootstrap 返回 5000 条记录'
  sleep 2
  for i in $(seq 10); do
    base=$(max_id)
    t0=$(ms)
    printf 'xpaste 测量 延迟 %s %s' "$i" "$t0" | pbcopy
    t=""
    while (($(ms) - t0 < 10000)); do
      if (($(max_id) > base)); then
        t=$(($(ms) - t0))
        break
      fi
      sleep 0.05
    done
    if [[ -n $t ]]; then samples="$samples $t"; else echo "第 $i 次 10 秒内未入库"; fi
    sleep 1
  done
  stop_app
  # shellcheck disable=SC2086
  summary 采集延迟 ms 1000 le 0 $samples
}

run_summon() {
  local i vals
  seed 5000
  launch
  wait_log 'bootstrap 返回 5000 条记录'
  sleep 1
  open_doc
  for i in $(seq 0 20); do
    front_ok
    key "$HOTKEY"
    sleep 0.6
    key "$HOTKEY"
    sleep 0.6
  done
  vals=$(log_values 唤起耗时 | paste -sd' ' -)
  stop_app
  echo "预热 ${vals%% *} ms（不计入）"
  # shellcheck disable=SC2086
  summary 唤起 ms 100 p95 20 ${vals#* }
}

run_search() {
  local k vals
  seed 5000
  launch
  wait_log 'bootstrap 返回 5000 条记录'
  sleep 1
  open_doc
  for _ in 1 2 3; do
    front_ok
    key "$HOTKEY"
    sleep 0.6
    # 0 0 4 2，再退格 4 次
    for k in 29 29 21 19 51 51 51 51; do
      key "key code $k"
      sleep 0.2
    done
    sleep 0.4
    key "$HOTKEY"
    sleep 0.6
  done
  vals=$(log_values 搜索耗时)
  stop_app
  # shellcheck disable=SC2086
  summary 搜索 ms 50 le 24 $vals
}

run_crash() {
  local i a b ok integrity
  seed 0
  launch
  wait_log 'bootstrap 返回'
  sleep 1
  for i in 1 2 3; do
    printf 'xpaste 测量 崩溃 %s %s' "$i" "$(ms)" | pbcopy
    sleep 1
  done
  for i in $(seq 100); do
    (($(q 'select count(*) from records') < 3)) || break
    sleep 0.1
  done
  echo "kill -9 前记录数 $(q 'select count(*) from records')"
  stop_app KILL
  launch
  ok=是
  wait_log 'bootstrap 返回 3 条记录' || ok=否
  integrity=$(q 'pragma integrity_check')
  [[ $integrity == ok ]] || ok=否
  echo "结果 强杀恢复 bootstrap=$(log_count 'bootstrap 返回 3 条记录') integrity_check=$integrity 达标=$ok"

  # 置顶第一条并复制，正常退出后再启动，两次导出应一致
  open_doc
  front_ok
  key "$HOTKEY"
  sleep 0.6
  key 'key code 35 using {command down}'
  sleep 0.5
  key 'key code 36'
  sleep 1
  # 模拟粘贴成功时面板已自行隐藏；只复制时面板还在
  if (($(log_count 已模拟粘贴) == 0)); then
    front_ok
    key "$HOTKEY"
  fi
  sleep 0.6
  a=$(q 'select id, pinned, uses, copied_at from records order by id')
  stop_app TERM
  launch
  wait_log 'bootstrap 返回 3 条记录' 30 2 || true
  b=$(q 'select id, pinned, uses, copied_at from records order by id')
  stop_app
  ok=是
  [[ $a == "$b" ]] || ok=否
  [[ $(q 'select count(*) from records where pinned = 1') == 1 && $(q 'select sum(uses) from records') -ge 1 ]] || ok=否
  echo "结果 退出持久化 前后一致=$([[ $a == "$b" ]] && echo 是 || echo 否) 达标=$ok"
  echo "  退出前："
  echo "$a" | sed 's/^/    /'
  echo "  重启后："
  echo "$b" | sed 's/^/    /'
}

run_idle() {
  local i pids fp="" rss="" cpu="" line
  seed 5000
  launch
  wait_log 'bootstrap 返回 5000 条记录'
  find_kids
  open_doc
  front_ok
  key "$HOTKEY"
  sleep 0.6
  key "$HOTKEY"
  echo "空闲 600 秒后采样…"
  sleep 600
  pids=$(paste -sd, "$ROOT/pids")
  echo "采样进程：$pids"
  for i in $(seq 6); do
    line=$(ps -o pid=,rss=,pcpu= -p "$pids" | awk '{ r += $2; c += $3 } END { printf "%.1f %.1f", r / 1024, c }' || true)
    rss="$rss ${line% *}"
    cpu="$cpu ${line#* }"
    # shellcheck disable=SC2046
    fp="$fp $(footprint --noCategories -f bytes $(sed 's/^/-p /' "$ROOT/pids") 2>/dev/null |
      awk '/\[[0-9]+\]:.*Footprint:/ { for (i = 1; i < NF; i++) if ($i == "Footprint:") s += $(i + 1) } END { printf "%.1f", s / 1048576 }' || true)"
    if ((i < 6)); then sleep 10; fi
  done
  stop_app
  # shellcheck disable=SC2086
  summary '内存(footprint)' MB 150 le 0 $fp
  # shellcheck disable=SC2086
  summary '内存(RSS)' MB - le 0 $rss
  # shellcheck disable=SC2086
  summary CPU % 1 lt 0 $cpu
}

echo "xPaste 测量 $(date '+%F %T')  $APP"
for step in $STEPS; do
  echo "== $step =="
  "run_$step"
done
