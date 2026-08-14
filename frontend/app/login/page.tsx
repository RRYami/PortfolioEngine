import type { Metadata } from "next";
import AuthForm from "@/app/components/AuthForm";

export const metadata: Metadata = {
  title: "PortfolioEngine · Sign in",
};

export default function LoginPage() {
  return <AuthForm mode="login" />;
}
