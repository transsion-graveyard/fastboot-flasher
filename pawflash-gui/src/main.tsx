import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import { FlashProgressProvider } from '@/hooks/useFlashProgress'
import { ForceFastbootProvider } from '@/hooks/useForceFastboot'
import App from './App.tsx'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <FlashProgressProvider>
      <ForceFastbootProvider>
        <App />
      </ForceFastbootProvider>
    </FlashProgressProvider>
  </StrictMode>,
)
