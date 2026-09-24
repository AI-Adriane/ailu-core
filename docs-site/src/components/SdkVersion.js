import useDocusaurusContext from "@docusaurus/useDocusaurusContext";

/** The SDK version these docs describe, read from packages/graph-sdk/package.json at build time. */
export default function SdkVersion() {
  const { siteConfig } = useDocusaurusContext();
  return siteConfig.customFields.sdkVersion;
}
