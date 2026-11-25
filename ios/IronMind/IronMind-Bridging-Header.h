//
//  IronMind-Bridging-Header.h
//  钢心 / Iron Mind —— Rust 内核（crates/sim-ios）的 C 接口声明。
//
//  这是 Swift 和 Rust 之间**唯一**的契约。改 Rust 那边 `#[no_mangle]` 的名字或
//  参数，这里必须同时改：C 没有重载、没有类型检查，签名对不上不会编译失败，
//  只会在运行期变成一串乱码或者直接崩。
//
//  故意不用 Swift Package 再包一层：一个头文件就够，Xcode 工程里少一个依赖，
//  也就少一处"为什么 package 里那个版本和仓库里的不一样"。
//

#ifndef IRONMIND_BRIDGING_HEADER_H
#define IRONMIND_BRIDGING_HEADER_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/// 出站 HTTP 的传输实现，由 Swift 的 URLSession 提供（见 HttpBridge.swift）。
///
/// - `url` / `key` / `body`：NUL 结尾的 UTF-8；`key` 是 Bearer 令牌（可以为空）；
/// - `timeout_secs`：超时秒数；
/// - `out` / `out_len`：响应体缓冲区，`out_len` 是**含结尾 NUL** 的容量；
/// - 返回：HTTP 状态码（>= 100），或负数表示传输层失败。
///
/// 对应 Rust 的 `sim_server::http::BridgeFn`。
typedef int32_t (*ironmind_http_fn)(
    void *ctx,
    const char *url,
    const char *key,
    const char *body,
    int32_t timeout_secs,
    char *out,
    size_t out_len);

/// 起内核。成功返回监听端口（> 0），失败返回 0 并把原因写进 `err_buf`。
///
/// `module_path` 传 NULL = 用打进静态库里的那个战役。
uint16_t ironmind_start(const char *data_dir,
                        const char *module_path,
                        uint16_t preferred_port,
                        char *err_buf,
                        size_t err_len);

/// 装（`f` = NULL 时卸掉）出站 HTTP 传输。
void ironmind_set_http_post(ironmind_http_fn f, void *ctx);

/// 内核版本号。**不要 free**（静态数据）。
const char *ironmind_version(void);

#ifdef __cplusplus
}
#endif

#endif /* IRONMIND_BRIDGING_HEADER_H */
