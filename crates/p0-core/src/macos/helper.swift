// p0 Keychain + Touch ID helper (pattern from ~/Coding/pay keystore).
// Payloads travel on stdin/stdout only, never argv.
import Foundation
import Security
import LocalAuthentication

let svc = "p0"

func main() {
    let a = CommandLine.arguments
    guard a.count >= 3 else { fail("usage: p0-keychain <store|read-protected|exists|delete> <account> [reason]") }
    switch a[1] {
    case "store": doStore(account: a[2], data: FileHandle.standardInput.readDataToEndOfFile())
    case "read-protected":
        doAuthenticate(reason: a.count >= 4 ? a[3] : "unlock p0 vault")
        doRead(account: a[2])
    case "exists": doExists(account: a[2])
    case "delete": doDelete(account: a[2])
    default: fail("unknown command \(a[1])")
    }
}

func base(_ account: String) -> [String: Any] {
    [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: svc, kSecAttrAccount as String: account]
}

func doStore(account: String, data: Data) {
    SecItemDelete(base(account) as CFDictionary)
    var q = base(account)
    q[kSecValueData as String] = data
    q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
    let s = SecItemAdd(q as CFDictionary, nil)
    guard s == errSecSuccess else { fail(errMsg(s)) }
}

func doRead(account: String) {
    var q = base(account)
    q[kSecReturnData as String] = true
    var r: AnyObject?
    let s = SecItemCopyMatching(q as CFDictionary, &r)
    guard s == errSecSuccess, let d = r as? Data else { fail(errMsg(s)) }
    FileHandle.standardOutput.write(d)
}

func doExists(account: String) {
    let s = SecItemCopyMatching(base(account) as CFDictionary, nil)
    print(s == errSecSuccess ? "yes" : "no")
}

func doDelete(account: String) {
    let s = SecItemDelete(base(account) as CFDictionary)
    guard s == errSecSuccess || s == errSecItemNotFound else { fail(errMsg(s)) }
}

func doAuthenticate(reason: String) {
    let ctx = LAContext()
    var e: NSError?
    // Biometrics, falling back to the login password if Touch ID is unavailable.
    let policy: LAPolicy = ctx.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &e)
        ? .deviceOwnerAuthenticationWithBiometrics : .deviceOwnerAuthentication
    let sema = DispatchSemaphore(value: 0)
    var err: String? = nil
    ctx.evaluatePolicy(policy, localizedReason: reason) { ok, e in
        if !ok { err = e?.localizedDescription ?? "denied" }
        sema.signal()
    }
    sema.wait()
    if let err = err { fail("auth: \(err)") }
}

func errMsg(_ s: OSStatus) -> String { SecCopyErrorMessageString(s, nil) as String? ?? "error \(s)" }
func fail(_ m: String) -> Never { FileHandle.standardError.write("ERROR:\(m)\n".data(using: .utf8)!); exit(1) }

main()
