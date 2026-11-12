import type { SignupMode } from "@/lib/hub/api";

/**
 * What each sign-up mode says and offers. The values are translation keys: the
 * copy itself lives with the other page words, so this file only decides which
 * sentence a mode draws.
 */
const ACCOUNT_PRICING_DESCRIPTION =
  "Use Droppen on one device with no account, free with an account, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.";

type SignupCopy = {
  heroNote: string | null;
  waitlistLabel: string | null;
  planNote: string;
  waitlistTitle: string;
  waitlistLead: string;
  joined: string;
  freeCta: string;
  pricingDescription: string;
};

export const SIGNUP_COPY: Record<SignupMode, SignupCopy> = {
  open: {
    heroNote: null,
    waitlistLabel: null,
    planNote: "Sign up with an e-mail address",
    waitlistTitle: "Leave your address, and we'll write when Pro opens",
    waitlistLead: "One e-mail, when it opens: nothing else, and never more than 12 months.",
    joined: "You're on the list: we'll e-mail you an invite.",
    freeCta: "Start free",
    pricingDescription: ACCOUNT_PRICING_DESCRIPTION,
  },
  invite: {
    heroNote: "Invite-only for now: sign-up needs an invite code.",
    waitlistLabel: "No code yet? Join the waitlist",
    planNote: "Sign-up is by invite for now",
    waitlistTitle: "Leave your address, and we'll write when it's your turn",
    waitlistLead: "We'll write when it's your turn.",
    joined: "You're on the list: we'll e-mail you an invite.",
    freeCta: "Start free",
    pricingDescription:
      "Use Droppen on one device with no account, free with an invite, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.",
  },
  closed: {
    heroNote: "Sign-up is closed for now.",
    waitlistLabel: "Join the waitlist",
    planNote: "Sign-up is closed for now.",
    waitlistTitle: "Leave your address, and we'll write when sign-up or Pro opens",
    waitlistLead: "We'll write once, when sign-up opens or Pro does.",
    joined: "You're on the list: we'll e-mail you when sign-up opens.",
    freeCta: "Join the waitlist",
    pricingDescription: ACCOUNT_PRICING_DESCRIPTION,
  },
};
