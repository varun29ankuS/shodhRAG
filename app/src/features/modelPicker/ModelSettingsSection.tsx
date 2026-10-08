import { useId } from 'react';
import { useModelPicker } from './modelApi';
import { ConnectPanel } from './ConnectPanel';
import { ModelPicks } from './ModelPicks';
import { AdvancedModelSettings } from './AdvancedModelSettings';

/**
 * Settings → Model, in three parts: Connect (a subscription, an API key or
 * a model on this computer), Model (four picks from what is connected) and
 * Advanced (collapsed).
 */
export function ModelSettingsSection() {
  const picker = useModelPicker();
  const connectId = useId();
  const modelId = useId();
  return (
    <div className="flex flex-col gap-5">
      <section className="p-5 rounded-[14px] bg-shodh-surface border border-shodh-border flex flex-col gap-4" aria-labelledby={connectId}>
        <h3 id={connectId} className="m-0 text-[15px] font-semibold text-shodh-text">
          Connect
        </h3>
        <ConnectPanel picker={picker} />
      </section>
      <section className="p-5 rounded-[14px] bg-shodh-surface border border-shodh-border flex flex-col gap-3" aria-labelledby={modelId}>
        <h3 id={modelId} className="m-0 text-[15px] font-semibold text-shodh-text">
          Model
        </h3>
        <ModelPicks picker={picker} />
      </section>
      <AdvancedModelSettings picker={picker} />
    </div>
  );
}
