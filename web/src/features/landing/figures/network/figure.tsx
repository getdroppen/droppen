import { useTranslation } from "react-i18next";
import { Figure } from "../figure";
import { NETWORK } from "./model";
import { NetworkLive } from "./live";

export function NetworkFigure() {
  const { t } = useTranslation();
  const title = t("How it connects");
  const labels = {
    direct: t("Direct · {{ms}}", { ms: `${NETWORK.direct[0]}–${NETWORK.direct[1]} ms` }),
    relay: t("Relay · {{ms}}", { ms: `${NETWORK.relay[0]}–${NETWORK.relay[1]} ms` }),
    relayName: t("Droppen relay"),
    computer: t("This device"),
    phone: t("Phone"),
  };

  return (
    <Figure
      title={title}
      keys={[
        { mark: "dim", label: labels.direct },
        { mark: "lit", label: labels.relay },
        { mark: "block", label: labels.relayName },
        { mark: "payload", label: t("Encrypted data only") },
      ]}
      caption={<>{t("Switches within 1.5 s")}. {t("Keys you type are sent again in order")}.</>}
    >
      <NetworkLive label={title} labels={labels} />
    </Figure>
  );
}
