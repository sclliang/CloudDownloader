import pan123ShareDomains from "./pan123-share-domains.txt?raw";

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
  { id: "quark", name: "夸克网盘", initial: "夸", color: "#397cf3" },
  { id: "baidu", name: "百度网盘", initial: "百", color: "#388cf8" },
  { id: "pan115", name: "115 网盘", initial: "115", color: "#287bd7" },
  { id: "pan123", name: "123 云盘", initial: "123", color: "#3671e4" },
  { id: "c139", name: "139 云盘", initial: "139", color: "#27a380" },
  { id: "uc", name: "UC 网盘", initial: "UC", color: "#e18b3b" },
  { id: "lanzou", name: "蓝奏云", initial: "蓝", color: "#428ccf" },
  { id: "ilanzou", name: "蓝奏云优享版", initial: "优", color: "#319ba7" },
];
export const anonymousShare = (id: Provider) =>
  ["uc", "lanzou", "ilanzou"].includes(id);
export const providerName = (id: Provider) =>
  providers.find((p) => p.id === id)?.name || id;

const pan123ShareSites = new Set(pan123ShareDomains.trim().split(/\s+/));

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
        pan123ShareSites.has(host.replace(/^www\./, "")) ||
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
