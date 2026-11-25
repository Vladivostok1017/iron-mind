import Foundation

/// 把内核的**出站** HTTPS 接到 URLSession 上。
///
/// 为什么需要这一层：内核调大模型原本是 `Command::new("curl")`（桌面、安卓都这么跑）。
/// iOS 上既没有 `/usr/bin/curl`，App 也被禁止 fork/exec 别的二进制——不接这一层，
/// 将领就只剩本地规则判断：游戏照常能玩，但"LLM 将领"这个特性等于没有，
/// 而且**不会有任何报错**，看起来就像将领变木了。
///
/// 线程模型：Rust 在**后台线程**里同步等结果（`llm::Broker` / `HqDriver` 的 worker
/// 线程），这里用信号量把它挂住，等 URLSession 的回调。所以千万不能阻塞主线程——
/// 内核里所有模型请求都走 worker 线程，这条前提成立。
enum HttpBridge {

    /// 装一次就够。`ironmind_start` 之前调用。
    static func install() {
        ironmind_set_http_post(httpPost, nil)
    }

    /// C 回调。
    ///
    /// 必须是**不捕获任何上下文**的闭包：C 函数指针没有闭包环境，
    /// 捕了上下文就编不过（Swift 会直接报 "a C function pointer cannot be formed"）。
    private static let httpPost: @convention(c) (
        UnsafeMutableRawPointer?,
        UnsafePointer<CChar>?,
        UnsafePointer<CChar>?,
        UnsafePointer<CChar>?,
        Int32,
        UnsafeMutablePointer<CChar>?,
        Int
    ) -> Int32 = { _, urlPtr, keyPtr, bodyPtr, timeoutSecs, outPtr, outLen in
        guard let urlPtr, let keyPtr, let bodyPtr, let outPtr, outLen > 0 else { return -1 }
        return post(url: String(cString: urlPtr),
                    key: String(cString: keyPtr),
                    body: String(cString: bodyPtr),
                    timeoutSecs: Int(timeoutSecs),
                    out: outPtr,
                    outLen: outLen)
    }

    /// 同步发一个 POST，把响应体写进 `out`，返回 HTTP 状态码（< 0 = 传输失败）。
    private static func post(url: String,
                             key: String,
                             body: String,
                             timeoutSecs: Int,
                             out: UnsafeMutablePointer<CChar>,
                             outLen: Int) -> Int32 {
        guard let target = URL(string: url) else { return -1 }

        var req = URLRequest(url: target, timeoutInterval: TimeInterval(timeoutSecs))
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if !key.isEmpty {
            req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = Data(body.utf8)

        let done = DispatchSemaphore(value: 0)
        var status: Int32 = -1
        var payload = Data()

        let task = URLSession.shared.dataTask(with: req) { data, response, error in
            defer { done.signal() }
            // URLSession 的回调在它自己的后台队列上，不是主线程——这条很重要，
            // 因为调用方这会儿正卡在信号量上。
            if error != nil { return }
            if let http = response as? HTTPURLResponse { status = Int32(http.statusCode) }
            payload = data ?? Data()
        }
        task.resume()

        // 多等 3 秒：URLSession 的超时和内核那边的超时是两条独立计时器，
        // 谁先到都不该让这里读到一个还没写完的缓冲区。
        if done.wait(timeout: .now() + .seconds(max(5, timeoutSecs + 3))) == .timedOut {
            task.cancel()
            return -1
        }

        // 写回去：NUL 结尾，超长截断（内核给的是 1 MB，模型回复通常几百字节）。
        let n = min(payload.count, outLen - 1)
        if n > 0 {
            payload.withUnsafeBytes { raw in
                guard let base = raw.baseAddress else { return }
                out.withMemoryRebound(to: UInt8.self, capacity: outLen) { dst in
                    dst.update(from: base.assumingMemoryBound(to: UInt8.self), count: n)
                }
            }
        }
        out[n] = 0
        return status
    }
}
