import { cn } from '@/shared/lib/utils';

export function KpiTile({
  label,
  value,
  hint,
  dotClassName,
  isActive,
  onClick,
}: {
  label: string;
  value: number | null;
  hint: string;
  dotClassName: string;
  isActive: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={isActive}
      className={cn(
        'flex flex-col items-start gap-half rounded-sm border bg-secondary p-double text-left',
        'transition-colors cursor-pointer hover:border-brand/60',
        'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand',
        isActive ? 'border-brand' : 'border-border'
      )}
    >
      <span className="flex items-center gap-half text-base text-low">
        <span
          className={cn('size-2 rounded-full', dotClassName)}
          aria-hidden="true"
        />
        {label}
      </span>
      <span className="text-3xl font-semibold leading-none tabular-nums text-high">
        {value ?? '–'}
      </span>
      <span className="text-sm text-low">{hint}</span>
    </button>
  );
}
