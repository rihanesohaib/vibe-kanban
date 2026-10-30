import type { ReactNode } from 'react';
import { cn } from '@/shared/lib/utils';

export function Panel({
  title,
  actions,
  className,
  children,
}: {
  title: ReactNode;
  actions?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section
      className={cn(
        'flex min-w-0 flex-col rounded-sm border border-border bg-secondary',
        className
      )}
    >
      <header className="flex items-center gap-base border-b border-border px-double py-base">
        <h2 className="text-lg font-medium text-high">{title}</h2>
        {actions && <div className="ml-auto flex items-center">{actions}</div>}
      </header>
      <div className="min-h-0 flex-1 p-double">{children}</div>
    </section>
  );
}

export function PanelMessage({ children }: { children: ReactNode }) {
  return <p className="py-double text-center text-base text-low">{children}</p>;
}
