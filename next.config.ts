import type { NextConfig } from "next";

// Tauri embeds the UI as static files: export statically and turn off server-side image optimization.
const nextConfig: NextConfig = {
  output: "export",
  images: { unoptimized: true },
  reactStrictMode: true,
  devIndicators: false,
};

export default nextConfig;
