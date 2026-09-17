import Link from "next/link";

export default function NotFound() {
  return (
    <div className="plate p-6">
      <p className="label">404</p>
      <p className="mt-2 text-sm">
        <Link href="/" className="underline" style={{ color: "var(--signal)" }}>
          ar-OCG-Router
        </Link>
      </p>
    </div>
  );
}
