import { useTranslation } from "react-i18next";
import { Figure } from "../figure";
import { KeyLive } from "./live";

export function KeyFigure() {
  const { t } = useTranslation();
  const title = t("End-to-end encryption");
  const labels = {
    ciphertext: t("Encrypted data only"),
    keys: t("The key stays on this device"),
    opened: t("Decrypted on your phone"),
  };

  return (
    <Figure
      title={title}
      keys={[{ mark: "key", label: t("Keys") }, { mark: "payload", label: labels.ciphertext }]}
      caption={t("Keys are exchanged only between your devices and never pass through Droppen Hub or a relay.")}
    >
      <KeyLive label={title} labels={labels} />
    </Figure>
  );
}
