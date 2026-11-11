import Foundation
import CryptoKit

// Sparkle's generate_keys exports a base64-encoded 32-byte private seed.
let environment = ProcessInfo.processInfo.environment
func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}
guard let encoded = environment["SPARKLE_PRIVATE_KEY"],
      let seed = Data(base64Encoded: encoded.trimmingCharacters(in: .whitespacesAndNewlines)),
      seed.count == 32,
      let expected = environment["SPARKLE_PUBLIC_KEY"],
      let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: seed) else {
    fail("Set SPARKLE_PRIVATE_KEY to a 32-byte seed exported by Sparkle generate_keys -x, and set SPARKLE_PUBLIC_KEY.")
}
guard key.publicKey.rawRepresentation.base64EncodedString() == expected else {
    fail("SPARKLE_PRIVATE_KEY does not match SPARKLE_PUBLIC_KEY.")
}
