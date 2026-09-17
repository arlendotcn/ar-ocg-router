/** @type {import('next').NextConfig} */
const nextConfig = {
  // The router embeds web/out into the binary and serves it as static files, so the build
  // must be a fully static export: no server runtime, no image optimizer, no middleware.
  output: "export",
  trailingSlash: true,
  images: { unoptimized: true },
  reactStrictMode: true,
  eslint: { ignoreDuringBuilds: true },
};

export default nextConfig;
