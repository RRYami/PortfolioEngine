import type { Metadata } from "next";
import AuthForm from "@/app/components/AuthForm";

export const metadata: Metadata = {
  title: "PortfolioEngine · Create account",
};

export default function RegisterPage() {
  return <AuthForm mode="register" />;
}
