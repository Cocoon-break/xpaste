# 官网部署（nginx）

一个域名、一个 nginx，做两件事：

- 提供 `website/` 里的静态页面（首页、购买页、隐私说明、演示）。
- 把请求转给授权与收款服务（Issuance）：
  - 浏览器的 `/api/…` → 授权服务**内网端口**的 `/v1/site/…`，由 nginx 加上站点密钥。
  - 应用的 `/v1/activate`、`/v1/deactivate`、`/v1/devices` 和支付渠道的 `/v1/webhooks/…` → 授权服务**公网端口**，原样转发。
  - 其余 `/api/`、`/v1/` 一律 404。授权服务的 `/v1/site/`、`/admin`、`/openapi.yaml` 从这个域名访问不到。

域名还没定，所以下面凡是 `<域名>` 的地方都等域名下来再填；域名之外的东西都在这个目录里。

## 目录

| 文件 | 作用 |
| --- | --- |
| `docker-compose.yml` | nginx 官方镜像，挂载 `website/` 和下面的模板；另有一个只在签证书时用的 certbot |
| `.env.example` | 环境变量样例，复制成 `.env` 再填（`.env` 已被 `.gitignore` 挡住） |
| `nginx/xpaste-http.inc.template` | 上游、限流区、缓存头 |
| `nginx/xpaste-site.inc.template` | 站点本体：静态文件、安全头、`/api/` 和 `/v1/` 的转发规则 |
| `nginx/http.conf.template` | `NGINX_MODE=http`：只听 80 |
| `nginx/tls.conf.template` | `NGINX_MODE=tls`：80 跳转到 443，用 certbot 签的证书 |

模板里 `${大写}` 的是环境变量，nginx 官方镜像启动时替换；`$小写` 的是 nginx 自己的变量。

## 从零部署

### 1. 授权服务这边

授权服务（Issuance）按它自己的 README 部署好，两个端口：公网端口（默认 8080）、内网端口（默认 8081）。然后在管理后台（内网端口的 `/admin`）：

1. 「应用」页新建应用：标识 `xpaste`，授权码前缀 `XP`。
2. 建商品和价格（比如 `pro-1x`：买断、大版本 1、2 台设备、CNY 99.00 / USD 14.99）。商品可以有多个，购买页按接口返回的来画。
3. 「系统设置」里配好邮件服务器（不配就发不出授权码邮件）和收款渠道。生产环境不要开 `MOCK_PROVIDER`。
4. 在「应用」页拿两样东西：
   - **站点密钥** → 填到这里的 `.env`（`SITE_SECRET`）。
   - **公钥**（Ed25519，base64）→ 交给应用那边，编译进应用用来验证授权凭证。
5. 授权服务的环境变量 `TRUSTED_PROXIES` 填上这台 nginx 的地址（授权服务看到的对端地址，不确定就看授权服务访问日志里的 `ip` 字段），改完重启授权服务。**不填的话所有用户会共用一个限流额度**：授权服务按客户端 IP 限流，只有请求来自可信代理时才读 `X-Forwarded-For`。

内网端口上有站点接口和管理后台，**不能暴露到公网**。nginx 跑在容器里、授权服务跑在宿主机上时，内网端口要监听容器够得着的地址，同时用防火墙挡住外网对这个端口的访问。

### 2. 起 nginx

```sh
cd deploy/site
cp .env.example .env
# 编辑 .env：SITE_SECRET、ISSUANCE_INTERNAL、ISSUANCE_PUBLIC；有域名了再填 SERVER_NAME
docker compose up -d
```

默认是 HTTP 模式，起来之后先按下面的“自查清单”过一遍。

### 3. HTTPS

二选一：

**做法 A（默认推荐）：本机 certbot 签证书，nginx 自己终结 HTTPS。**

```sh
# 先保持 NGINX_MODE=http、HTTP_PORT=80、SERVER_NAME=<域名>，域名解析指到这台机器，然后签证书：
docker compose run --rm certbot certonly --webroot -w /var/www/certbot -d <域名> --email <你的邮箱> --agree-tos --no-eff-email

# 证书签出来之后，把 .env 里的 NGINX_MODE 改成 tls，重建容器：
docker compose up -d

# 续期：证书 90 天有效，每周跑一次（放进 crontab）
docker compose run --rm certbot renew && docker compose exec nginx nginx -s reload
```

`tls` 模式下 `SERVER_NAME` 只填一个域名（证书路径和 80 → 443 的跳转都用它）。HSTS 默认没开，在 `nginx/tls.conf.template` 里有一行注释，确认 HTTPS 稳定后再打开。

**做法 B：HTTPS 由前面另一层终结（云负载均衡、CDN）。** 保持 `NGINX_MODE=http`。这时 nginx 看到的对端是前面那一层，要在 `nginx/http.conf.template` 的 server 块里加上 `set_real_ip_from <前一层的地址段>;` 和 `real_ip_header X-Forwarded-For;`，否则按 IP 的限流会把所有访客算成一个人。

### 4. 域名定了之后要填的地方

1. `.env` 的 `SERVER_NAME`。
2. 支付渠道后台的回调地址：`https://<域名>/v1/webhooks/<渠道名>`（渠道名和 `/api/providers` 返回的 `name` 一致）。
3. GitHub 仓库变量 `XPASTE_SITE` = `https://<域名>`（不带末尾斜杠）。应用编译时读它：授权接口是 `${XPASTE_SITE}/v1/…`，购买页是 `${XPASTE_SITE}/buy.html`。
4. 把公钥交给应用那边（见上面第 1 节）。
5. `website/index.html` 里的 `og:url` 和 `og:image` 现在写的是 GitHub Pages 的地址，改成新域名。
6. 官网切过来之后的收尾：
   - 仓库里指向 `https://cocoon-break.github.io/xpaste/` 的链接（`README.md`、`CLAUDE.md`、`CONTRIBUTING.md` 等）改成新域名。
   - `.github/workflows/pages.yml`：可以留着（GitHub Pages 上的旧地址继续能看，购买页在那里显示“购买暂未开放”），也可以停掉；想让旧地址跳到新域名，需要在 GitHub Pages 那边另外处理，这里没有准备。
   - 下载链接指向 GitHub Releases，**不需要动**。页面里的接口和资源都是相对地址，**不需要动**。

## 自查清单

把 `B` 换成站点地址后逐条跑；第一列是应该看到的状态码。

```sh
B=https://<域名>

# —— 应该通 ——
curl -s -o /dev/null -w '%{http_code}\n' $B/                    # 200
curl -s -o /dev/null -w '%{http_code}\n' $B/buy.html            # 200
curl -s -o /dev/null -w '%{http_code}\n' $B/demo/index.html     # 200
curl -s $B/api/products                                         # 200，商品列表
curl -s $B/api/providers                                        # 200，渠道列表
curl -s -X POST -H 'Content-Type: application/json' -d '{}' $B/v1/activate   # 400，授权服务回的 JSON（说明转到了）

# —— 必须 404 ——
for p in /api/ /api/licenses /api/admin /v1/site/products /v1/ /admin /admin/api/orders /openapi.yaml; do
  curl -s -o /dev/null -w "%{http_code} $p\n" $B$p
done

# —— 方法不对：405 ——
curl -s -o /dev/null -w '%{http_code}\n' -X POST $B/api/products
curl -s -o /dev/null -w '%{http_code}\n' $B/api/orders

# —— 浏览器自己带的 Authorization 不起作用：仍然 200 ——
curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer whatever' $B/api/products

# —— 限流：连发 12 次下单，后面几次应该是 429（用不合法的邮箱，不会真的产生订单） ——
for i in $(seq 12); do curl -s -o /dev/null -w '%{http_code} ' -H 'Content-Type: application/json' -d '{"product":"x","email":"x","provider":"x","currency":"CNY"}' $B/api/orders; done; echo
```

再用浏览器走一遍：首页的 Pro 一节显示价格和“购买 Pro”；演示面板能动；购买页能下单；付款后页面自动显示授权码；点“打开 xPaste 并激活”能拉起应用。浏览器控制台里不应该有内容安全策略（CSP）的报错。

限流的取值在 `nginx/xpaste-http.inc.template`：查询每 IP 每秒 5 次（可突发 20 次），下单和找回授权码每 IP 每分钟 6 次（可突发 5 次）。

## 更新网站内容

`website/` 是直接挂载进容器的，文件名不带哈希，页面和脚本每次都会向服务器确认有没有变（图片缓存一天）：

```sh
git pull          # 页面改动立即生效，不用重启
```

改了 `nginx/` 下的模板或 `.env`：

```sh
docker compose up -d --force-recreate nginx
```

接入新的收款渠道时，在 `website/assets/buy-core.js` 的 `PROVIDER_LABELS` 里给它加一个中文叫法（不加就显示渠道名本身）。

## 购买流程的纯函数测试

```sh
node --test 'scripts/site/*.test.js'     # make test / make check 也会带上
```

## 已知的取舍

- **CSP 带 `'unsafe-inline'`**：首页的样式和脚本是内联的，演示页也是。要去掉得把内联脚本挪成文件或加哈希，这次没做。
- **`return_url` 带不上订单号**：订单号要下单之后才有。购买页把进行中的订单记在浏览器的本地存储里，从渠道跳回 `buy.html` 时靠它接着查；如果渠道在另一个浏览器里打开回跳地址，就接不上，买家要靠邮件里的授权码。
- **没有传 `client_ip`**：这个字段要由可信的后端填，nginx 只做转发、改不了请求体，浏览器自己填的又不可信。等接入需要它的渠道时再处理（比如在授权服务那边改成读 `X-Forwarded-For`）。
- **订单号等于取授权码的凭据**：`GET /api/orders/<订单号>` 在付款后会返回授权码。订单号是 32 位随机十六进制，猜不出来，但不要把它贴到公开的地方。
