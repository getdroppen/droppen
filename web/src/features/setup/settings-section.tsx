import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { SettingRow, SettingsSection } from "@/features/settings/layout";
import { isDesktopApp } from "@/lib/auth";
import { useHomeMachine, useMachine } from "@/lib/machines/context";

/**
 * Settings → General → Setup, in the Droppen app: runs the first-run guide
 * again from step 01 (`/welcome?rerun=1`).
 * What is set up stays set up; each step shows it as it is now.
 */
export function SetupSection() {
  const navigate = useNavigate();
  const machine = useMachine();
  const home = useHomeMachine();
  if (!isDesktopApp() || machine !== home) return null;
  return <SetupSectionView onRerun={() => void navigate({ to: "/welcome", search: { rerun: 1 } })} />;
}

/** The section itself (the render tests draw it directly). */
export function SetupSectionView({ onRerun }: { onRerun: () => void }) {
  const { t } = useTranslation();
  return (
    <SettingsSection title={t("Setup")} data-testid="setup-section">
      <SettingRow tight label={t("Check Claude Code and Codex, change where you use Droppen, pick a first folder again.")}>
        <Button data-testid="setup-rerun" onClick={onRerun}>
          {t("Run setup again")}
        </Button>
      </SettingRow>
    </SettingsSection>
  );
}
