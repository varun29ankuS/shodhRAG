import React, { useState } from 'react';
import { useTheme } from '../contexts/ThemeContext';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Sparkles,
  FolderOpen,
  Zap,
  CheckCircle,
  ChevronRight,
  ChevronLeft,
  Code,
  Shield,
  Activity,
  Rocket,
} from 'lucide-react';

interface OnboardingFlowProps {
  isOpen: boolean;
  onComplete: () => void;
  onSkip: () => void;
}

interface Step {
  id: string;
  title: string;
  description: string;
  icon: React.ReactNode;
  features?: string[];
  action?: {
    label: string;
    onClick: () => void;
  };
}

export function OnboardingFlow({ isOpen, onComplete, onSkip }: OnboardingFlowProps) {
  const { colors } = useTheme();
  const [currentStep, setCurrentStep] = useState(0);

  const steps: Step[] = [
    {
      id: 'welcome',
      title: 'Welcome to SHODH',
      description: 'Your AI-powered document intelligence platform. Let\'s get you set up in 3 simple steps.',
      icon: <Sparkles className="w-16 h-16" />,
      features: [
        'Index and search any document collection',
        'AI-powered Q&A over your documents',
        'Hybrid search: semantic + keyword',
        'Local-first, private by default',
      ],
    },
    {
      id: 'features',
      title: 'Powerful Document Intelligence',
      description: 'SHODH helps you search, understand, and extract insights from your documents.',
      icon: <img src="/shodh_logo_nobackground.svg" alt="SHODH" className="w-16 h-16" />,
      features: [
        'Natural language document queries',
        'PDF, DOCX, XLSX, PPTX, and more',
        'Source citations with every answer',
        'Multi-document cross-referencing',
        'Contextual chunking for precision',
      ],
    },
    {
      id: 'workspace',
      title: 'Index Your First Documents',
      description: 'Point SHODH to a folder of documents to get started. Everything stays on your machine.',
      icon: <FolderOpen className="w-16 h-16" />,
      features: [
        'Supports PDF, DOCX, XLSX, PPTX, TXT, MD, HTML, CSV',
        'Indexes locally (your documents never leave your machine)',
        'Fast incremental updates',
        'Multiple document collections supported',
      ],
    },
    {
      id: 'ready',
      title: 'You\'re All Set!',
      description: 'SHODH is ready to help you search and understand your documents.',
      icon: <Rocket className="w-16 h-16" />,
      features: [
        'Try: "Summarize the key findings"',
        'Try: "What does the report say about revenue?"',
        'Try: "Compare these two documents"',
        'Drag and drop files to index them instantly',
      ],
    },
  ];

  const currentStepData = steps[currentStep];
  const isLastStep = currentStep === steps.length - 1;
  const isFirstStep = currentStep === 0;

  const handleNext = () => {
    if (isLastStep) {
      handleComplete();
    } else {
      setCurrentStep(currentStep + 1);
    }
  };

  const handleBack = () => {
    if (!isFirstStep) {
      setCurrentStep(currentStep - 1);
    }
  };

  const handleComplete = () => {
    localStorage.setItem('onboarding_completed', 'true');
    localStorage.setItem('onboarding_completed_at', new Date().toISOString());
    onComplete();
  };

  const handleSkipAll = () => {
    localStorage.setItem('onboarding_skipped', 'true');
    onSkip();
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-6" style={{ background: 'rgba(0, 0, 0, 0.9)' }}>
      <div className="w-full max-w-3xl">
        <AnimatePresence mode="wait">
          <motion.div
            key={currentStep}
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -20 }}
            transition={{ duration: 0.3 }}
            className="rounded-2xl shadow-2xl border-2 overflow-hidden"
            style={{ background: colors.bg, borderColor: colors.border }}
          >
            {/* Progress Bar */}
            <div className="h-1.5" style={{ background: colors.bgTertiary }}>
              <motion.div
                className="h-full"
                style={{ background: colors.primary }}
                initial={{ width: 0 }}
                animate={{ width: `${((currentStep + 1) / steps.length) * 100}%` }}
                transition={{ duration: 0.3 }}
              />
            </div>

            {/* Content */}
            <div className="p-12 text-center">
              {/* Icon */}
              <motion.div
                initial={{ scale: 0.5, opacity: 0 }}
                animate={{ scale: 1, opacity: 1 }}
                transition={{ delay: 0.1, duration: 0.3 }}
                className="mx-auto mb-8 flex items-center justify-center"
                style={{ color: colors.primary }}
              >
                {currentStepData.icon}
              </motion.div>

              {/* Title */}
              <motion.h2
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                transition={{ delay: 0.2 }}
                className="text-3xl font-bold mb-4"
                style={{ color: colors.text }}
              >
                {currentStepData.title}
              </motion.h2>

              {/* Description */}
              <motion.p
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                transition={{ delay: 0.3 }}
                className="text-lg mb-8 max-w-2xl mx-auto"
                style={{ color: colors.textSecondary }}
              >
                {currentStepData.description}
              </motion.p>

              {/* Features */}
              {currentStepData.features && (
                <motion.div
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  transition={{ delay: 0.4 }}
                  className="max-w-xl mx-auto mb-8"
                >
                  <div className="grid gap-3 text-left">
                    {currentStepData.features.map((feature, index) => (
                      <motion.div
                        key={feature}
                        initial={{ opacity: 0, x: -20 }}
                        animate={{ opacity: 1, x: 0 }}
                        transition={{ delay: 0.5 + index * 0.1 }}
                        className="flex items-start gap-3 p-3 rounded-lg"
                        style={{ background: colors.bgSecondary }}
                      >
                        <CheckCircle className="w-5 h-5 flex-shrink-0 mt-0.5" style={{ color: colors.success }} />
                        <span style={{ color: colors.text }}>{feature}</span>
                      </motion.div>
                    ))}
                  </div>
                </motion.div>
              )}

              {/* Action Button (if present) */}
              {currentStepData.action && (
                <motion.button
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  transition={{ delay: 0.6 }}
                  onClick={currentStepData.action.onClick}
                  className="px-8 py-4 rounded-lg font-semibold text-lg mb-8"
                  style={{ background: colors.primary, color: '#ffffff' }}
                >
                  {currentStepData.action.label}
                </motion.button>
              )}
            </div>

            {/* Navigation */}
            <div className="p-6 border-t flex items-center justify-between" style={{ background: colors.bgSecondary, borderColor: colors.border }}>
              {/* Step Indicator */}
              <div className="flex items-center gap-2">
                {steps.map((_, index) => (
                  <div
                    key={index}
                    className="h-2 w-2 rounded-full transition-all"
                    style={{
                      background: index === currentStep ? colors.primary : colors.border,
                      width: index === currentStep ? '2rem' : '0.5rem',
                    }}
                  />
                ))}
              </div>

              {/* Buttons */}
              <div className="flex items-center gap-3">
                {!isLastStep && (
                  <button
                    onClick={handleSkipAll}
                    className="px-4 py-2 text-sm font-medium transition-all"
                    style={{ color: colors.textMuted }}
                  >
                    Skip for now
                  </button>
                )}

                {!isFirstStep && (
                  <button
                    onClick={handleBack}
                    className="px-6 py-3 rounded-lg font-medium transition-all flex items-center gap-2 border-2"
                    style={{
                      background: 'transparent',
                      borderColor: colors.border,
                      color: colors.text,
                    }}
                  >
                    <ChevronLeft className="w-4 h-4" />
                    Back
                  </button>
                )}

                <button
                  onClick={handleNext}
                  className="px-6 py-3 rounded-lg font-medium transition-all flex items-center gap-2"
                  style={{
                    background: colors.primary,
                    color: '#ffffff',
                  }}
                >
                  {isLastStep ? (
                    <>
                      Get Started
                      <Zap className="w-4 h-4" />
                    </>
                  ) : (
                    <>
                      Next
                      <ChevronRight className="w-4 h-4" />
                    </>
                  )}
                </button>
              </div>
            </div>
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
