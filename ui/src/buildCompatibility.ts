export type BuildCompatibility = "same_build" | "compatible_android" | "different_build";

export function comparePeerBuild(local: string, peer: string, peerOs?: string): BuildCompatibility {
  if (peer === local) return "same_build";
  if (!peerOs?.toLowerCase().includes("android")) return "different_build";

  const localParts = local.split(" · ");
  const peerParts = peer.split(" · ");
  const localProtocol = localParts.at(-1);
  const peerProtocol = peerParts.at(-1);
  const compatible = localParts[0] === peerParts[0]
    && /^control-v\d+$/.test(localProtocol ?? "")
    && localProtocol === peerProtocol;
  return compatible ? "compatible_android" : "different_build";
}
