import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Figure } from "../figure";
import { KeyLive } from "./live";

/*
 * The key boundary as a line: a sealed capsule on the conveyor, a keyed station
 * at each end, a relay gantry whose socket is empty, and the archive that keeps
 * encrypted copies. The copy says the one thing the drawing shows but cannot
 * spell out: the keys never cross.
 */
export function KeyFigure() {
  const { t } = useTranslation();

  const labels = useMemo(
    () => ({
      keys: t("Keys"),
      copy: t("Droppen Hub · encrypted copy"),
      opened: t("Decrypted on your phone"),
      ciphertext: t("Encrypted data only"),
    }),
    [t],
  );
  const [readout, setReadout] = useState(labels.ciphertext);
  useEffect(() => setReadout(labels.ciphertext), [labels.ciphertext]);

  return (
    <Figure
      title={t("End-to-end encryption")}
      readout={readout}
      keys={[
        { mark: "key", label: t("Keys") },
        { mark: "dashed", label: t("The key stays on this device") },
      ]}
      caption={t("Keys are exchanged only between your devices and never pass through Droppen Hub or a relay.")}
    >
      <KeyLive label={t("End-to-end encryption")} labels={labels} onReadout={setReadout} />
    </Figure>
  );
}
