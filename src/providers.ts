export type Provider =
  | "quark"
  | "baidu"
  | "pan115"
  | "pan123"
  | "c139"
  | "uc"
  | "lanzou"
  | "ilanzou";
export type CloudProvider = Exclude<Provider, "baidu">;

export const providers: {
  id: Provider;
  name: string;
  initial: string;
  color: string;
}[] = [
  { id: "quark", name: "夸克网盘", initial: "夸", color: "#7165c8" },
  { id: "baidu", name: "百度网盘", initial: "百", color: "#6784c8" },
  { id: "pan115", name: "115 网盘", initial: "115", color: "#7d6ac0" },
  { id: "pan123", name: "123 云盘", initial: "123", color: "#8a70bd" },
  { id: "c139", name: "139 云盘", initial: "139", color: "#769d93" },
  { id: "uc", name: "UC 网盘", initial: "UC", color: "#bd845f" },
  { id: "lanzou", name: "蓝奏云", initial: "蓝", color: "#6286ba" },
  { id: "ilanzou", name: "蓝奏云优享版", initial: "优", color: "#658fa3" },
];
export const anonymousShare = (id: Provider) =>
  ["uc", "lanzou", "ilanzou"].includes(id);
export const providerName = (id: Provider) =>
  providers.find((p) => p.id === id)?.name || id;

export function shareProvider(text: string): Provider | undefined {
  for (const match of text.matchAll(/https?:\/\/[^\s<>"，。；）)\]]+/g)) {
    try {
      const url = new URL(match[0]);
      if (url.username || url.password || url.port) continue;
      const host = url.hostname;
      if (host === "pan.quark.cn") return "quark";
      if (host === "pan.baidu.com") return "baidu";
      if (
        [
          "115.com",
          "www.115.com",
          "115cdn.com",
          "www.115cdn.com",
          "115rc.com",
          "www.115rc.com",
          "anxia.com",
        ].includes(host)
      )
        return "pan115";
      if (
        [
          "123pan.com",
          "www.123pan.com",
          "123pan.cn",
          "www.123pan.cn",
          "123865.com",
          "www.123865.com",
          "123912.com",
          "www.123912.com",
        ].includes(host) ||
        host === "share.123pan.cn" ||
        host.endsWith(".share.123pan.cn")
      )
        return "pan123";
      if (host === "yun.139.com") return "c139";
      if (["drive.uc.cn", "fast.uc.cn"].includes(host)) return "uc";
      if (["ilanzou.com", "www.ilanzou.com"].includes(host)) return "ilanzou";
      if (
        /^(?:[a-z0-9-]+\.)*(?:lanzou[a-z]?|lan[zs]o[ux])\.(?:com|net|org|cn)$/.test(
          host,
        )
      )
        return "lanzou";
    } catch {
      /* The backend reports malformed share links. */
    }
  }
}
