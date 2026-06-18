import { Button } from '@heroui/react';

export function App() {
  return (
    <main className="min-h-screen bg-slate-50 text-gray-950">
      <section className="mx-auto flex min-h-screen w-full max-w-4xl flex-col items-start justify-center gap-8 px-6 py-12">
        <div className="space-y-4">
          <p className="text-sm font-medium uppercase tracking-normal text-gray-500">HeroUI v3 demo</p>
          <h1 className="text-4xl font-semibold tracking-normal sm:text-5xl">
            Binder Trace WebUI
          </h1>
          <p className="max-w-2xl text-base leading-7 text-gray-600">
            The previous interface has been removed. This page is a minimal React 19,
            Tailwind CSS v4, and HeroUI setup smoke test.
          </p>
        </div>

        <div className="flex flex-wrap gap-3">
          <Button>HeroUI primary</Button>
          <Button variant="secondary">Secondary action</Button>
          <Button variant="outline">Outline action</Button>
        </div>
      </section>
    </main>
  );
}
