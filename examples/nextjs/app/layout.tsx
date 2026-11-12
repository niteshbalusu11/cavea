import type { ReactNode } from "react";

export const metadata = { title: "Choose your seats" };

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body
        style={{
          margin: "0 auto",
          maxWidth: 1100,
          padding: 16,
          fontFamily: "system-ui, sans-serif",
        }}
      >
        {children}
      </body>
    </html>
  );
}
