import "@fontsource/be-vietnam-pro/300.css";
import "@fontsource/be-vietnam-pro/400.css";
import "@fontsource/be-vietnam-pro/500.css";
import "@fontsource/be-vietnam-pro/600.css";
import "@fontsource/be-vietnam-pro/700.css";
import "@fontsource/be-vietnam-pro/800.css";
import "@fontsource-variable/montserrat/index.css";
import "../styles/tokens.css";
import "../styles/base.css";
import "../styles/decor.css";
import "../styles/header.css";
import "../styles/pages.css";
import "../styles/buttons.css";
import "../styles/controls.css";
import "../styles/cards.css";
import "../styles/overlay.css";
import "../styles/home.css";
import "../styles/settings.css";
import "../styles/cursor.css";
import "../styles/preview.css";
import "../styles/wallpaper.css";
import "../styles/labels.css";
import "../styles/taskbar.css";
import "../styles/features.css";
import "../styles/motion.css";

export const metadata = { title: "ShunguangDecor" };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
