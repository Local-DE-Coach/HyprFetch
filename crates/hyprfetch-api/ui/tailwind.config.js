/** @type {import('tailwindcss')} */
import daisyui from 'daisyui'

export default {
  content: ['./index.html', './src/**/*.{svelte,js}'],
  plugins: [daisyui],
  daisyui: {
    // 5 theme styles × dark/light (v0.5.0): custom COLORFUL palettes.
    // The stock themes were replaced because the app looked plain white in
    // light mode / plain gray in dark mode; every base is now tinted and
    // every primary/secondary/accent is vivid. Theme NAMES stay identical
    // ('dim','light','night','winter','forest','garden','coffee','autumn',
    // 'synthwave','valentine') so choices stored on the server keep working.
    // All sets are static CSS variables — zero runtime cost, RAM-friendly.
    themes: [
      // Indigo (style id: slate) — vivid indigo on lavender / deep indigo navy
      {
        light: {
          colorScheme: 'light',
          'base-100': '#f6f5ff', 'base-200': '#ecebfe', 'base-300': '#dcd9fb',
          'base-content': '#322f55',
          primary: '#6d5bf6', 'primary-content': '#ffffff',
          secondary: '#e85bb8', 'secondary-content': '#ffffff',
          accent: '#0fb5d6', 'accent-content': '#04303a',
          neutral: '#3a3663', 'neutral-content': '#f5f4ff',
          info: '#3f8cff', 'info-content': '#ffffff',
          success: '#14a56b', 'success-content': '#ffffff',
          warning: '#f7b955', 'warning-content': '#3e2c00',
          error: '#e5484d', 'error-content': '#ffffff',
        },
        dim: {
          colorScheme: 'dark',
          'base-100': '#191831', 'base-200': '#15142b', 'base-300': '#232247',
          'base-content': '#dcdcf5',
          primary: '#8d7bfa', 'primary-content': '#16123d',
          secondary: '#f07ec6', 'secondary-content': '#33102a',
          accent: '#35d0e8', 'accent-content': '#062a30',
          neutral: '#2c2b4b', 'neutral-content': '#dcdcf5',
          info: '#6aa5ff', 'info-content': '#0a1e3d',
          success: '#3ed598', 'success-content': '#062e1c',
          warning: '#ffc05e', 'warning-content': '#3a2700',
          error: '#ff7077', 'error-content': '#3d0a0d',
        },
      },
      // Ocean (style id: ocean) — sky blue on seafoam / deep ocean navy
      {
        winter: {
          colorScheme: 'light',
          'base-100': '#f2f9ff', 'base-200': '#e3f1fd', 'base-300': '#cfe6f8',
          'base-content': '#1d3a52',
          primary: '#0e8fd5', 'primary-content': '#ffffff',
          secondary: '#6366f1', 'secondary-content': '#ffffff',
          accent: '#14b8a6', 'accent-content': '#ffffff',
          neutral: '#23425c', 'neutral-content': '#f2f9ff',
          info: '#0284c7', 'info-content': '#ffffff',
          success: '#16a34a', 'success-content': '#ffffff',
          warning: '#d97706', 'warning-content': '#ffffff',
          error: '#dc2626', 'error-content': '#ffffff',
        },
        night: {
          colorScheme: 'dark',
          'base-100': '#0c1b2c', 'base-200': '#0a1626', 'base-300': '#14283e',
          'base-content': '#d3e5f5',
          primary: '#4cc3fa', 'primary-content': '#06222f',
          secondary: '#8b93f8', 'secondary-content': '#141438',
          accent: '#2dd4bf', 'accent-content': '#042f2b',
          neutral: '#1c3350', 'neutral-content': '#d3e5f5',
          info: '#58b6ff', 'info-content': '#061c30',
          success: '#35d08e', 'success-content': '#062b1a',
          warning: '#ffb457', 'warning-content': '#3a2700',
          error: '#ff6b6e', 'error-content': '#3d0a0d',
        },
      },
      // Forest (style id: forest) — emerald on mint / deep pine
      {
        garden: {
          colorScheme: 'light',
          'base-100': '#f2fbf4', 'base-200': '#e2f6e7', 'base-300': '#cdecd6',
          'base-content': '#1e3d2a',
          primary: '#199f58', 'primary-content': '#ffffff',
          secondary: '#7cb518', 'secondary-content': '#14200a',
          accent: '#0d9488', 'accent-content': '#ffffff',
          neutral: '#274c37', 'neutral-content': '#f2fbf4',
          info: '#0284c7', 'info-content': '#ffffff',
          success: '#16a34a', 'success-content': '#ffffff',
          warning: '#ca8a04', 'warning-content': '#ffffff',
          error: '#dc2626', 'error-content': '#ffffff',
        },
        forest: {
          colorScheme: 'dark',
          'base-100': '#0f1f16', 'base-200': '#0c1a12', 'base-300': '#172d20',
          'base-content': '#d2ecdc',
          primary: '#52d983', 'primary-content': '#08250f',
          secondary: '#a3e635', 'secondary-content': '#1a2403',
          accent: '#34d399', 'accent-content': '#062b1a',
          neutral: '#1d382a', 'neutral-content': '#d2ecdc',
          info: '#5bb7f5', 'info-content': '#07202f',
          success: '#3fd58e', 'success-content': '#062b1a',
          warning: '#ffc45e', 'warning-content': '#3a2700',
          error: '#ff6f72', 'error-content': '#3d0a0d',
        },
      },
      // Sunset (style id: coffee) — warm amber on cream / espresso
      {
        autumn: {
          colorScheme: 'light',
          'base-100': '#fffaf0', 'base-200': '#fdf0da', 'base-300': '#f8e2c2',
          'base-content': '#4a3520',
          primary: '#e07c00', 'primary-content': '#ffffff',
          secondary: '#e05a8a', 'secondary-content': '#ffffff',
          accent: '#8b5cf6', 'accent-content': '#ffffff',
          neutral: '#52402c', 'neutral-content': '#fffaf0',
          info: '#2f7fd4', 'info-content': '#ffffff',
          success: '#199f58', 'success-content': '#ffffff',
          warning: '#e78a05', 'warning-content': '#ffffff',
          error: '#dc4444', 'error-content': '#ffffff',
        },
        coffee: {
          colorScheme: 'dark',
          'base-100': '#211710', 'base-200': '#1c130e', 'base-300': '#2f2218',
          'base-content': '#f3e4d3',
          primary: '#ffb224', 'primary-content': '#331c00',
          secondary: '#ff8fab', 'secondary-content': '#38141f',
          accent: '#c084fc', 'accent-content': '#2a0a45',
          neutral: '#3a2c20', 'neutral-content': '#f3e4d3',
          info: '#62b0f8', 'info-content': '#0a2033',
          success: '#43d592', 'success-content': '#062b1a',
          warning: '#ffc45e', 'warning-content': '#3a2700',
          error: '#ff7077', 'error-content': '#3d0a0d',
        },
      },
      // Neon (style id: cyber) — fuchsia/violet glow
      {
        valentine: {
          colorScheme: 'light',
          'base-100': '#fdf3ff', 'base-200': '#f9e3fd', 'base-300': '#f2cff9',
          'base-content': '#43204f',
          primary: '#c026d3', 'primary-content': '#ffffff',
          secondary: '#0ea5e9', 'secondary-content': '#ffffff',
          accent: '#7c3aed', 'accent-content': '#ffffff',
          neutral: '#4a2b55', 'neutral-content': '#fdf3ff',
          info: '#0284c7', 'info-content': '#ffffff',
          success: '#16a34a', 'success-content': '#ffffff',
          warning: '#d97706', 'warning-content': '#ffffff',
          error: '#dc2626', 'error-content': '#ffffff',
        },
        synthwave: {
          colorScheme: 'dark',
          'base-100': '#1b1038', 'base-200': '#160c30', 'base-300': '#281a4d',
          'base-content': '#e9d9fa',
          primary: '#e879f9', 'primary-content': '#2f0a37',
          secondary: '#22d3ee', 'secondary-content': '#062a30',
          accent: '#a78bfa', 'accent-content': '#1e1043',
          neutral: '#2c1e50', 'neutral-content': '#e9d9fa',
          info: '#60a5fa', 'info-content': '#0a1e3d',
          success: '#34d399', 'success-content': '#062b1a',
          warning: '#fbbf24', 'warning-content': '#3a2700',
          error: '#fb7185', 'error-content': '#3d0a0d',
        },
      },
    ],
    logs: false,
  },
}
